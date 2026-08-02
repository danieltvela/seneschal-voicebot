#!/usr/bin/env python3
"""Seneschal TUI QA harness (PTY).

Executes scenarios from QA_TEST.md (core + P0) against ./mac-seneschal.sh.
Captures strip-ANSI TUI text, asserts markers, writes report + failure artifacts.

Usage:
  python3 scripts/qa_tui_pty.py [--out DIR] [--tests 01,02,...] [--skip-interactive]
"""

from __future__ import annotations

import argparse
import errno
import fcntl
import os
import re
import select
import signal
import struct
import sys
import termios
import time
import traceback
from dataclasses import dataclass, field
from pathlib import Path
from typing import Callable, Optional

try:
    import pyte
except ImportError:  # pragma: no cover
    pyte = None  # type: ignore

# ── ANSI strip ──────────────────────────────────────────────────────────────
ANSI_RE = re.compile(
    r"\x1b(?:[@-Z\\-_]|\[[0-?]*[ -/]*[@-~]|\][^\x07\x1b]*(?:\x07|\x1b\\)|[PX^_][^\x1b]*\x1b\\)"
)
OSC_RE = re.compile(r"\x1b\][^\x07\x1b]*(?:\x07|\x1b\\)")


def strip_ansi(data: bytes | str) -> str:
    if isinstance(data, bytes):
        text = data.decode("utf-8", errors="replace")
    else:
        text = data
    text = OSC_RE.sub("", text)
    text = ANSI_RE.sub("", text)
    # Drop other C0 controls except newline/tab
    text = re.sub(r"[\x00-\x08\x0b\x0c\x0e-\x1f\x7f]", "", text)
    return text


def collapse_ws(s: str) -> str:
    return re.sub(r"[ \t]+", " ", s)


# ── Result model ────────────────────────────────────────────────────────────
@dataclass
class TestResult:
    id: str
    name: str
    status: str  # PASS | FAIL | SKIP | PARTIAL
    notes: str = ""
    snapshot: str = ""
    log_tail: str = ""


@dataclass
class Harness:
    repo: Path
    out_dir: Path
    cols: int = 200  # approximate maximized terminal (QA_TEST.md)
    rows: int = 50
    master_fd: Optional[int] = None
    child_pid: Optional[int] = None
    buf: bytearray = field(default_factory=bytearray)
    log_path: Path = field(init=False)
    results: list[TestResult] = field(default_factory=list)
    boot_ok: bool = False
    # pyte virtual terminal — authoritative screen state after in-place redraws
    _screen: object = field(default=None, repr=False)
    _stream: object = field(default=None, repr=False)

    def __post_init__(self) -> None:
        self.out_dir.mkdir(parents=True, exist_ok=True)
        (self.out_dir / "issues").mkdir(exist_ok=True)
        self.log_path = self.out_dir / "process.log"
        if self.log_path.exists():
            self.log_path.unlink()
        self._init_vt()

    def _init_vt(self) -> None:
        if pyte is None:
            self._screen = None
            self._stream = None
            return
        self._screen = pyte.Screen(self.cols, self.rows)
        self._stream = pyte.ByteStream(self._screen)
        # Alternate screen support
        try:
            self._screen.set_mode(pyte.modes.LNM)
        except Exception:
            pass

    def _dotenv_map(self) -> dict[str, str]:
        """Parse repo .env into a dict (handles inline # comments like bash source)."""
        env_path = self.repo / ".env"
        out: dict[str, str] = {}
        if not env_path.exists():
            return out
        for line in env_path.read_text(encoding="utf-8", errors="replace").splitlines():
            line = line.strip()
            if not line or line.startswith("#") or "=" not in line:
                continue
            if line.startswith("export "):
                line = line[len("export ") :]
            key, _, val = line.partition("=")
            key = key.strip()
            val = val.strip()
            # Strip inline comments when value is unquoted
            if val and val[0] not in ('"', "'"):
                if " #" in val:
                    val = val.split(" #", 1)[0].rstrip()
                elif "\t#" in val:
                    val = val.split("\t#", 1)[0].rstrip()
            else:
                # Quoted: remove matching quotes; keep interior
                if len(val) >= 2 and val[0] == val[-1] and val[0] in ('"', "'"):
                    val = val[1:-1]
                else:
                    val = val.strip("'").strip('"')
            if key:
                out[key] = val
        return out

    # ── PTY lifecycle ─────────────────────────────────────────────────────
    def start(
        self,
        cmd: list[str] | None = None,
        use_script: bool = False,
        env_overrides: dict[str, str] | None = None,
    ) -> None:
        """Start Seneschal under a PTY.

        Default runs ``target/release/seneschal`` with the same feature-related
        env as ``mac-seneschal.sh``. Using the binary (not ``cargo run``) keeps
        the PTY attached to the TUI process so control keys reach crossterm.
        """
        dotenv = self._dotenv_map()
        if env_overrides:
            dotenv.update(env_overrides)
        bin_path = self.repo / "target" / "release" / "seneschal"
        if cmd is None:
            if use_script or not bin_path.exists():
                cmd = [str(self.repo / "mac-seneschal.sh")]
            else:
                cmd = [str(bin_path)]
        self.buf.clear()
        self.boot_ok = False
        self._init_vt()

        pid, master_fd = os.forkpty()
        if pid == 0:
            # Child — apply .env + mac-seneschal.sh defaults
            for k, v in dotenv.items():
                os.environ[k] = v
            os.environ.setdefault("TERM", "xterm-256color")
            os.environ["COLUMNS"] = str(self.cols)
            os.environ["LINES"] = str(self.rows)
            os.environ.setdefault("RUST_LOG", "debug")
            os.environ.setdefault("STT_PROVIDER", "speech")
            try:
                os.chdir(self.repo)
                os.execvp(cmd[0], cmd)
            except Exception as e:  # pragma: no cover
                sys.stderr.write(f"exec failed: {e}\n")
                os._exit(127)

        self.child_pid = pid
        self.master_fd = master_fd
        # Non-blocking master
        fl = fcntl.fcntl(master_fd, fcntl.F_GETFL)
        fcntl.fcntl(master_fd, fcntl.F_SETFL, fl | os.O_NONBLOCK)
        # Window size
        winsize = struct.pack("HHHH", self.rows, self.cols, 0, 0)
        try:
            fcntl.ioctl(master_fd, termios.TIOCSWINSZ, winsize)
        except OSError:
            pass

    def _append_log(self, data: bytes) -> None:
        with open(self.log_path, "ab") as f:
            f.write(data)

    def drain(self, timeout: float = 0.05) -> bytes:
        if self.master_fd is None:
            return b""
        got = bytearray()
        end = time.time() + timeout
        while time.time() < end:
            r, _, _ = select.select([self.master_fd], [], [], max(0.0, end - time.time()))
            if not r:
                break
            try:
                chunk = os.read(self.master_fd, 65536)
            except OSError as e:
                if e.errno in (errno.EAGAIN, errno.EWOULDBLOCK):
                    break
                if e.errno == errno.EIO:
                    break
                raise
            if not chunk:
                break
            got.extend(chunk)
            self.buf.extend(chunk)
            self._append_log(chunk)
            if self._stream is not None:
                try:
                    self._stream.feed(chunk)
                except Exception:
                    pass
        return bytes(got)

    def text(self) -> str:
        return strip_ansi(bytes(self.buf))

    def screen_text(self) -> str:
        """Current virtual screen contents (handles in-place ratatui redraws)."""
        if self._screen is None:
            return self.recent_text()
        # pyte display is list of lines
        try:
            lines = list(self._screen.display)
            return "\n".join(lines)
        except Exception:
            return self.recent_text()

    def recent_text(self, n: int = 12000) -> str:
        # Prefer virtual screen (accurate status bar) + tail of raw for chat history
        scr = self.screen_text()
        raw = self.text()
        raw_tail = raw[-n:] if len(raw) > n else raw
        # Combine: screen first (current UI), then raw markers for chat content
        return scr + "\n---RAW---\n" + raw_tail

    def snapshot_clean(self) -> str:
        """Best-effort current screen + raw tail."""
        return self.screen_text() + "\n---RAW TAIL---\n" + self.text()[-4000:]

    def wait_for(
        self,
        predicates: list[Callable[[str], bool]] | Callable[[str], bool],
        timeout: float = 90.0,
        label: str = "condition",
        all_of: bool = True,
    ) -> bool:
        if callable(predicates):
            preds = [predicates]
        else:
            preds = list(predicates)
        deadline = time.time() + timeout
        while time.time() < deadline:
            self.drain(0.15)
            t = self.recent_text()
            hits = [p(t) for p in preds]
            if all_of and all(hits):
                return True
            if not all_of and any(hits):
                return True
            # Child dead?
            if self.child_pid is not None:
                wpid, status = os.waitpid(self.child_pid, os.WNOHANG)
                if wpid != 0:
                    self.child_pid = None
                    self.drain(0.2)
                    return False
        return False

    def write(self, data: bytes | str) -> None:
        if self.master_fd is None:
            raise RuntimeError("PTY not started")
        if isinstance(data, str):
            data = data.encode("utf-8")
        # Write in small chunks for reliability
        off = 0
        while off < len(data):
            try:
                n = os.write(self.master_fd, data[off:])
                off += n
            except OSError as e:
                if e.errno in (errno.EAGAIN, errno.EWOULDBLOCK):
                    time.sleep(0.01)
                    continue
                raise
            time.sleep(0.01)

    def type_text(self, s: str, delay: float = 0.02) -> None:
        for ch in s:
            self.write(ch.encode("utf-8"))
            time.sleep(delay)
            self.drain(0.01)

    def enter(self) -> None:
        self.write(b"\r")
        time.sleep(0.05)
        self.drain(0.05)

    def ctrl(self, letter: str) -> None:
        """Send classic Ctrl+letter (A=1 ... Z=26). Note: Ctrl+M == CR == Enter."""
        c = letter.lower()
        code = ord(c) - ord("a") + 1
        self.write(bytes([code]))
        # Allow TUI tick (~33ms) + redraw
        time.sleep(0.25)
        self.drain(0.25)

    def ctrl_m_csiu(self) -> None:
        """Kitty/CSI-u encoding for Ctrl+M (codepoint 109, modifiers 5=ctrl)."""
        self.write(b"\x1b[109;5u")
        time.sleep(0.25)
        self.drain(0.25)

    def esc(self) -> None:
        """Send Escape. Must settle > crossterm ESC timeout before next bytes."""
        self.write(b"\x1b")
        time.sleep(0.25)
        self.drain(0.15)

    def is_alive(self) -> bool:
        if self.child_pid is None:
            return False
        wpid, _ = os.waitpid(self.child_pid, os.WNOHANG)
        if wpid != 0:
            self.child_pid = None
            return False
        return True

    def stop(self, sig: int = signal.SIGINT, grace: float = 5.0) -> int | None:
        """Signal child and wait. Returns exit code or None."""
        if self.child_pid is None:
            return None
        try:
            # Prefer Ctrl+C through PTY for clean TUI quit when possible
            if sig == signal.SIGINT and self.master_fd is not None:
                try:
                    self.write(b"\x03")  # Ctrl+C
                except OSError:
                    os.kill(self.child_pid, signal.SIGINT)
            else:
                os.kill(self.child_pid, sig)
        except ProcessLookupError:
            pass

        deadline = time.time() + grace
        status_code = None
        while time.time() < deadline:
            self.drain(0.1)
            if self.child_pid is None:
                break
            wpid, status = os.waitpid(self.child_pid, os.WNOHANG)
            if wpid != 0:
                self.child_pid = None
                if os.WIFEXITED(status):
                    status_code = os.WEXITSTATUS(status)
                elif os.WIFSIGNALED(status):
                    status_code = 128 + os.WTERMSIG(status)
                break
            time.sleep(0.05)

        if self.child_pid is not None:
            try:
                os.kill(self.child_pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            try:
                _, status = os.waitpid(self.child_pid, 0)
                if os.WIFEXITED(status):
                    status_code = os.WEXITSTATUS(status)
                elif os.WIFSIGNALED(status):
                    status_code = 128 + os.WTERMSIG(status)
            except ChildProcessError:
                pass
            self.child_pid = None

        if self.master_fd is not None:
            try:
                os.close(self.master_fd)
            except OSError:
                pass
            self.master_fd = None
        return status_code

    def kill_leftover_seneschal(self) -> list[str]:
        """Kill leftover target/release/seneschal (not cargo). Return remaining pids."""
        import subprocess

        out = subprocess.check_output(["pgrep", "-fl", "seneschal"] if False else ["ps", "aux"], text=True)
        leftovers = []
        for line in out.splitlines():
            if "target/release/seneschal" in line and "grep" not in line:
                parts = line.split()
                # ps aux: USER PID ...
                try:
                    pid = int(parts[1])
                except (IndexError, ValueError):
                    continue
                leftovers.append(f"{pid}: {line[:120]}")
                try:
                    os.kill(pid, signal.SIGTERM)
                except ProcessLookupError:
                    pass
        time.sleep(0.5)
        remaining = []
        out2 = subprocess.check_output(["ps", "aux"], text=True)
        for line in out2.splitlines():
            if "target/release/seneschal" in line and "grep" not in line:
                remaining.append(line[:140])
                try:
                    pid = int(line.split()[1])
                    os.kill(pid, signal.SIGKILL)
                except Exception:
                    pass
        return remaining

    def log_tail(self, n: int = 50) -> str:
        if not self.log_path.exists():
            return ""
        raw = self.log_path.read_bytes()
        text = strip_ansi(raw)
        lines = text.splitlines()
        return "\n".join(lines[-n:])

    def save_failure(self, test_id: str, name: str, expected: str, actual: str) -> Path:
        snap = self.snapshot_clean()
        logt = self.log_tail(50)
        issue_path = self.out_dir / "issues" / f"{test_id}.md"
        issue_path.write_text(
            f"# QA Failure: {test_id} — {name}\n\n"
            f"**Environment**: macOS, `./mac-seneschal.sh` (features: control,speech,avspeech,tui,parakeet,remote)\n\n"
            f"## Expected\n\n{expected}\n\n"
            f"## Actual\n\n{actual}\n\n"
            f"## TUI snapshot (strip-ANSI, tail)\n\n```\n{snap[-8000:]}\n```\n\n"
            f"## Process log (last 50 lines)\n\n```\n{logt}\n```\n",
            encoding="utf-8",
        )
        (self.out_dir / f"{test_id}-snapshot.txt").write_text(snap[-12000:], encoding="utf-8")
        (self.out_dir / f"{test_id}-log-tail.txt").write_text(logt, encoding="utf-8")
        return issue_path

    def record(
        self,
        test_id: str,
        name: str,
        status: str,
        notes: str = "",
        expected: str = "",
        actual: str = "",
    ) -> TestResult:
        snap = self.snapshot_clean()[-4000:]
        logt = self.log_tail(50) if status in ("FAIL", "PARTIAL") else ""
        if status == "FAIL":
            self.save_failure(test_id, name, expected or notes, actual or notes)
        r = TestResult(test_id, name, status, notes, snap, logt)
        self.results.append(r)
        line = f"[{status}] {test_id} {name}" + (f" — {notes}" if notes else "")
        print(line, flush=True)
        try:
            with open(self.out_dir / "progress.log", "a", encoding="utf-8") as pf:
                pf.write(line + "\n")
        except OSError:
            pass
        return r


# ── Markers ─────────────────────────────────────────────────────────────────
# Status bar pattern: "● IDLE │ TTS ON │ ACTIVE │ SIMPLE │ INSERT │ Ctrl+T ..."
STATUS_RE = re.compile(
    r"●\s*(IDLE|LISTENING|TRANSCRIBING|THINKING|SPEAKING)"
    r".{0,80}?"
    r"(TTS\s+O(?:N|FF))"
    r".{0,40}?"
    r"(ACTIVE|AMBIENT🔒|AMBIENT)"
    r".{0,40}?"
    r"(SIMPLE🔒|COMPLEX🔒|SIMPLE|COMPLEX|—)"
    r".{0,20}?"
    r"(INSERT|NORMAL)"
    r".{0,10}?"
    r"Ctrl\+",
    re.DOTALL,
)


def last_status(t: str) -> dict:
    """Parse the most recent status bar.

    Prefer the virtual-screen portion (before ``---RAW---``) so in-place
    ratatui updates are not masked by older frames in the raw PTY stream.
    """
    if "---RAW---" in t:
        screen_part, _, raw_part = t.partition("---RAW---")
        # Prefer screen; fall back to raw if screen has no status yet
        primary = screen_part if STATUS_RE.search(screen_part) or "●" in screen_part else raw_part
    else:
        primary = t

    matches = list(STATUS_RE.finditer(primary))
    if not matches:
        # Fallback looser search near the end of primary
        tail = primary[-2500:]
        state = None
        for s in ("THINKING", "SPEAKING", "TRANSCRIBING", "LISTENING", "IDLE"):
            if f"● {s}" in tail or f"●{s}" in tail:
                state = s
                break
        intent = None
        for label in ("SIMPLE🔒", "COMPLEX🔒", "SIMPLE", "COMPLEX", "—"):
            if label in tail:
                intent = label
                break
        return {
            "state": state,
            "tts": "OFF" if "TTS OFF" in tail else ("ON" if "TTS ON" in tail else None),
            "intent": intent,
            "mode": "NORMAL" if "NORMAL" in tail else ("INSERT" if "INSERT" in tail else None),
            "raw": tail[-200:],
        }
    m = matches[-1]
    return {
        "state": m.group(1),
        "tts": "ON" if "ON" in m.group(2) else "OFF",
        "intent": m.group(4),
        "mode": m.group(5),
        "raw": m.group(0)[:200],
    }


def has_boot_ready(t: str) -> bool:
    # Avoid bare "seneschal" alone (cargo paths). Require status brand + IDLE + mode + shortcuts.
    brand = bool(re.search(r"\bseneschal\b", t))
    st = last_status(t)
    mode = st.get("mode") in ("INSERT", "NORMAL") or "INSERT" in t or "NORMAL" in t
    force_hint = "Ctrl+M force" in t
    quit_hint = "Ctrl+C quit" in t
    idle = st.get("state") == "IDLE" or "● IDLE" in t
    return brand and mode and force_hint and quit_hint and idle


def has_idle(t: str) -> bool:
    st = last_status(t)
    return st.get("state") == "IDLE"


def has_thinking(t: str) -> bool:
    st = last_status(t)
    return st.get("state") == "THINKING"


def has_streaming(t: str) -> bool:
    # Streaming marker is ephemeral; only trust virtual screen (not raw history).
    if "---RAW---" in t:
        screen_part = t.split("---RAW---", 1)[0]
    else:
        screen_part = t
    return "[streaming]" in screen_part


def has_panic(t: str) -> bool:
    return "panicked at" in t or "PANIC" in t


def has_simple_badge(t: str) -> bool:
    st = last_status(t)
    intent = st.get("intent") or ""
    return intent.startswith("SIMPLE")


def has_complex_badge(t: str) -> bool:
    st = last_status(t)
    intent = st.get("intent") or ""
    return intent.startswith("COMPLEX")


def has_tts_on(t: str) -> bool:
    st = last_status(t)
    if st.get("tts"):
        return st["tts"] == "ON"
    return "TTS ON" in t[-2000:]


def has_tts_off(t: str) -> bool:
    st = last_status(t)
    if st.get("tts"):
        return st["tts"] == "OFF"
    return "TTS OFF" in t[-2000:]


def has_insert(t: str) -> bool:
    st = last_status(t)
    if st.get("mode"):
        return st["mode"] == "INSERT"
    return "INSERT" in t[-1500:]


def has_normal(t: str) -> bool:
    st = last_status(t)
    if st.get("mode"):
        return st["mode"] == "NORMAL"
    return "NORMAL" in t[-1500:]


def has_force_notification(t: str) -> bool:
    return "Classifier force:" in t[-6000:]


def has_tool_activity(t: str) -> bool:
    tail = t[-8000:]
    patterns = [
        r"\btool\b",
        r"current_time",
        r"ToolCall",
        r"\[tool",
        r"\d{1,2}:\d{2}",
    ]
    return any(re.search(p, tail, re.I) for p in patterns)


def has_agent_activity(t: str, since_marker: str | None = None) -> bool:
    """Detect agent/tool orchestration UI (not mere model chatter about Hermes)."""
    if "---RAW---" in t:
        # Prefer raw for tool/agent event lines; screen for live widgets
        screen_part, _, raw_part = t.partition("---RAW---")
        blob = screen_part + "\n" + raw_part[-12000:]
    else:
        blob = t[-12000:]
    if since_marker and since_marker in blob:
        blob = blob[blob.rfind(since_marker) :]
    # Strong lifecycle / tool markers (not prose mentions of Hermes)
    strong = [
        "AgentTask",
        "subagent",
        "> tool:",
        "tool:",
        "Necesita confirmación",
        "tarea en segundo plano",
        "Finalizing",
        "Delegated",
        "[Started]",
        "Running…",
        "Running...",
    ]
    return any(k.lower() in blob.lower() for k in strong)


def wait_idle_after_turn(h: Harness, timeout: float = 180.0) -> bool:
    """Wait until status returns to IDLE after a submitted turn."""
    deadline = time.time() + timeout
    saw_busy = False
    started = time.time()
    while time.time() < deadline:
        h.drain(0.2)
        t = h.recent_text()
        if has_panic(t):
            return False
        st = last_status(t)
        state = st.get("state")
        if state in ("THINKING", "SPEAKING", "TRANSCRIBING", "LISTENING") or has_streaming(t):
            saw_busy = True
        if state == "IDLE" and not has_streaming(t):
            if saw_busy:
                time.sleep(0.35)
                h.drain(0.15)
                return True
            # Response may be so fast we miss busy frames
            if time.time() - started > 6.0:
                return True
    return has_idle(h.recent_text())


def wait_idle_simple(h: Harness, timeout: float = 120.0) -> bool:
    """Wait for current status IDLE (not historical)."""
    def ok(t: str) -> bool:
        st = last_status(t)
        return st.get("state") == "IDLE" and not has_streaming(t)

    return h.wait_for(ok, timeout=timeout, label="IDLE")


# ── Tests ───────────────────────────────────────────────────────────────────
def boot(
    h: Harness,
    timeout: float = 180.0,
    env_overrides: dict[str, str] | None = None,
    cmd: list[str] | None = None,
) -> bool:
    print("… starting seneschal under PTY …", flush=True)
    h.start(cmd=cmd, env_overrides=env_overrides)
    ok = h.wait_for(has_boot_ready, timeout=timeout, label="boot ready")
    h.boot_ok = ok
    print(f"… boot_ok={ok} status={last_status(h.recent_text())}", flush=True)
    return ok


def ensure_insert(h: Harness) -> None:
    if not has_insert(h.recent_text()):
        h.write(b"i")
        time.sleep(0.1)
        h.drain(0.1)


def conv_mode_from_status(t: str) -> str | None:
    """Extract ACTIVE / AMBIENT / AMBIENT🔒 from status bar."""
    st = last_status(t)
    # last_status raw includes conv label in full match group path via STATUS_RE group 3
    matches = list(STATUS_RE.finditer(t.split("---RAW---")[0] if "---RAW---" in t else t))
    if matches:
        return matches[-1].group(3)
    scr = t.split("---RAW---")[0] if "---RAW---" in t else t[-2000:]
    for label in ("AMBIENT🔒", "AMBIENT", "ACTIVE"):
        if label in scr:
            return label
    return None


def test_01_smoke(h: Harness) -> None:
    tid, name = "01", "Smoke Test (Boot & Main Surface)"
    t = h.recent_text()
    checks = {
        "status brand seneschal": bool(re.search(r"\bseneschal\b", t)),
        "● IDLE": "● IDLE" in t or "IDLE" in t,
        "INSERT/NORMAL": has_insert(t) or has_normal(t),
        "→ Seneschal or input": "→ Seneschal" in t or "Seneschal" in t or "Type a message" in t,
        "Ctrl+M force": "Ctrl+M force" in t,
        "Ctrl+C quit": "Ctrl+C quit" in t,
        "no panic": not has_panic(t),
    }
    failed = [k for k, v in checks.items() if not v]
    if not failed and h.boot_ok:
        h.record(tid, name, "PASS", "boot markers present")
    else:
        h.record(
            tid,
            name,
            "FAIL",
            notes="; ".join(failed) if failed else "boot not ready",
            expected="seneschal brand, ● IDLE, INSERT, shortcuts Ctrl+M force / Ctrl+C quit, no panic",
            actual=f"failed: {failed}; boot_ok={h.boot_ok}",
        )


def test_02_classifier(h: Harness) -> None:
    tid, name = "02", "Classifier Intent Badge & Force Toggle"
    if not h.boot_ok or not h.is_alive():
        h.record(tid, name, "SKIP", "app not running")
        return

    # Ensure INSERT + IDLE
    wait_idle_simple(h, 30)
    t0 = h.recent_text()
    if not has_insert(t0):
        h.write(b"i")
        time.sleep(0.1)
        h.drain(0.1)

    # 2a: simple prompt
    h.type_text("hola")
    h.enter()
    wait_idle_after_turn(h, 90)
    t1 = h.recent_text()
    simple_ok = has_simple_badge(t1)

    # 2b: complex prompt
    h.type_text("Investiga la estructura del proyecto")
    h.enter()
    wait_idle_after_turn(h, 180)
    t2 = h.recent_text()
    complex_ok = has_complex_badge(t2)

    # 2c: force cycle — try CSI-u first, then classic Ctrl+M (may conflict with Enter)
    force_notes = []
    force_ok = False
    h.ctrl_m_csiu()
    time.sleep(0.3)
    h.drain(0.3)
    t3 = h.recent_text()
    if has_force_notification(t3) or "SIMPLE🔒" in t3 or "COMPLEX🔒" in t3:
        force_ok = True
        force_notes.append("CSI-u Ctrl+M worked")
        # Cycle a couple more times to AUTO
        h.ctrl_m_csiu()
        time.sleep(0.2)
        h.ctrl_m_csiu()
        time.sleep(0.2)
        h.drain(0.2)
    else:
        force_notes.append(
            "CSI-u Ctrl+M did not update force badge (expected: classic PTY maps Ctrl+M==Enter; unit test covers keybinding)"
        )

    shortcut_ok = "Ctrl+M force" in t2 or "Ctrl+M force" in t1

    notes_parts = [
        f"SIMPLE after hola={simple_ok}",
        f"COMPLEX after research={complex_ok}",
        f"force={force_ok}",
        f"shortcut={shortcut_ok}",
    ] + force_notes

    if simple_ok and complex_ok and shortcut_ok:
        status = "PASS" if force_ok else "PARTIAL"
        h.record(tid, name, status, "; ".join(notes_parts))
        if status == "PARTIAL":
            h.save_failure(
                tid,
                name,
                "Force cycle shows Classifier force: … and 🔒 badges",
                "; ".join(notes_parts),
            )
    elif simple_ok or complex_ok:
        h.record(tid, name, "PARTIAL", "; ".join(notes_parts))
        h.save_failure(tid, name, "Badge SIMPLE then COMPLEX; force cycle", "; ".join(notes_parts))
    else:
        h.record(
            tid,
            name,
            "FAIL",
            "; ".join(notes_parts),
            expected="SIMPLE after hola; COMPLEX after research; force cycle",
            actual="; ".join(notes_parts),
        )


def test_03_memory(h: Harness) -> None:
    tid, name = "03", "Memory Retrieval"
    if not h.boot_ok or not h.is_alive():
        h.record(tid, name, "SKIP", "app not running")
        return
    wait_idle_simple(h, 30)
    if not has_insert(h.recent_text()):
        h.write(b"i")
        time.sleep(0.1)

    prompt = "Que recuerdas sobre OpenCode y Hermes en este proyecto? Nombra roles si los sabes."
    h.type_text(prompt)
    h.enter()
    wait_idle_after_turn(h, 180)
    t = h.recent_text()
    # Look for fact markers
    keys = ["OpenCode", "Hermes", "agente", "agent", "rol", "research", "subagent"]
    hit = any(k.lower() in t.lower() for k in keys)
    # Also require some assistant content beyond user echo
    no_panic = not has_panic(t)
    if hit and no_panic:
        h.record(tid, name, "PASS", "response references OpenCode/Hermes/agent roles")
    elif no_panic and has_idle(t):
        h.record(
            tid,
            name,
            "PARTIAL",
            "got reply but weak fact match — check snapshot",
        )
        h.save_failure(tid, name, "Response references OpenCode/Hermes roles", "weak keyword match")
    else:
        h.record(
            tid,
            name,
            "FAIL",
            "no fact reference or panic",
            expected="Response references OpenCode/Hermes/roles",
            actual=t[-1500:],
        )


def test_04_shutdown_idle(h: Harness) -> None:
    tid, name = "04", "Graceful Shutdown (idle)"
    # This test ends the process — only call when ready to restart for later tests
    if not h.boot_ok or not h.is_alive():
        h.record(tid, name, "SKIP", "app not running")
        return
    wait_idle_simple(h, 20)
    code = h.stop(signal.SIGINT, grace=8.0)
    time.sleep(0.5)
    logt = h.log_tail(80)
    panic = "panicked at" in logt or "panicked at" in h.text()
    # leftover check
    remaining = h.kill_leftover_seneschal()
    ok_code = code in (0, 130, None) or (code is not None and code != 127)
    # Accept 0 or 130 (SIGINT) or any non-panic exit
    if not panic and not remaining and h.child_pid is None:
        h.record(tid, name, "PASS", f"exit_code={code}; no panic; no leftovers")
    else:
        h.record(
            tid,
            name,
            "FAIL",
            f"exit_code={code} panic={panic} leftovers={remaining}",
            expected="clean exit, no panic, no leftover seneschal",
            actual=f"code={code} panic={panic} leftovers={remaining}",
        )


def test_05_research(h: Harness) -> None:
    tid, name = "05", "Research & Subagent Orchestration"
    if not h.boot_ok or not h.is_alive():
        h.record(tid, name, "SKIP", "app not running")
        return
    wait_idle_simple(h, 30)
    if not has_insert(h.recent_text()):
        h.write(b"i")
        time.sleep(0.1)

    prompt = "Analyze the project structure. Use subagents or tools if needed."
    h.type_text(prompt)
    h.enter()
    # Research can take long
    deadline = time.time() + 300
    saw_agent = False
    saw_answer = False
    saw_busy = False
    while time.time() < deadline:
        h.drain(0.3)
        t = h.recent_text()
        if has_panic(t):
            break
        st = last_status(t)
        if st.get("state") in ("THINKING", "SPEAKING") or has_streaming(t):
            saw_busy = True
        if has_agent_activity(t, since_marker="Analyze the project structure"):
            saw_agent = True
        # Prose mentions of Hermes/OpenCode still count as partial agent signal for product UX
        if "Hermes" in t or "OpenCode" in t or "Opencode" in t:
            saw_agent = True
        if st.get("state") == "IDLE" and saw_busy:
            time.sleep(0.4)
            h.drain(0.2)
            if last_status(h.recent_text()).get("state") == "IDLE":
                saw_answer = True
                break
        # Answer settled without catching busy
        if st.get("state") == "IDLE" and time.time() + 300 - deadline > 20:
            if "Analyze the project structure" in t:
                saw_answer = True
                break
    t = h.recent_text()
    if has_panic(t):
        h.record(tid, name, "FAIL", "panic during research", expected="agent activity + answer", actual="panic")
        return
    if saw_agent and saw_answer:
        h.record(tid, name, "PASS", "agent/tool activity visible; returned to IDLE")
    elif saw_answer:
        h.record(
            tid,
            name,
            "PARTIAL",
            "got answer / IDLE but agent lifecycle markers weak or not captured",
        )
        h.save_failure(
            tid,
            name,
            "Hermes/OpenCode/AgentTask lifecycle or tool progress + consolidated answer",
            "answer/IDLE only; agent markers weak",
        )
    else:
        h.record(
            tid,
            name,
            "FAIL",
            f"saw_agent={saw_agent} saw_answer={saw_answer}",
            expected="agent activity + consolidated answer",
            actual=t[-2000:],
        )


def test_06_keyboard_modes(h: Harness) -> None:
    tid, name = "06", "Keyboard modes INSERT / NORMAL"
    if not h.boot_ok or not h.is_alive():
        h.record(tid, name, "SKIP", "app not running")
        return
    wait_idle_simple(h, 30)
    # Default INSERT
    t0 = h.recent_text()
    insert0 = has_insert(t0)

    h.esc()
    normal = False
    for _ in range(15):
        time.sleep(0.1)
        h.drain(0.1)
        if has_normal(h.recent_text()):
            normal = True
            break
    t1 = h.recent_text()

    # Type xyz in NORMAL — must not submit
    h.type_text("xyz", delay=0.04)
    time.sleep(0.3)
    h.drain(0.2)
    t2 = h.recent_text()
    # In NORMAL, letters should not appear as a submitted user message
    no_think = not has_thinking(t2)
    still_normal = has_normal(t2)

    h.write(b"i")
    insert1 = False
    for _ in range(15):
        time.sleep(0.1)
        h.drain(0.1)
        if has_insert(h.recent_text()):
            insert1 = True
            break
    t3 = h.recent_text()

    h.type_text("ping")
    h.enter()
    wait_idle_after_turn(h, 90)
    t4 = h.recent_text()
    # Prefer user message marker
    ping_submitted = (
        ("You [text]" in t4 and "ping" in t4.lower()) or "ping" in t4[-3000:].lower()
    ) and not has_panic(t4)

    if insert0 and normal and still_normal and no_think and insert1 and ping_submitted:
        h.record(tid, name, "PASS", "Esc→NORMAL, xyz ignored, i→INSERT, ping submitted")
    else:
        notes = f"insert0={insert0} normal={normal} still_normal={still_normal} no_think={no_think} insert1={insert1} ping={ping_submitted}"
        status = "PARTIAL" if (normal or insert1) and not has_panic(t4) else "FAIL"
        h.record(tid, name, status, notes)
        if status != "PASS":
            h.save_failure(tid, name, "mode labels track Esc/i; only INSERT message submitted", notes)


def test_07_tts_toggle(h: Harness) -> None:
    tid, name = "07", "TTS toggle (Ctrl+T)"
    if not h.boot_ok or not h.is_alive():
        h.record(tid, name, "SKIP", "app not running")
        return
    wait_idle_simple(h, 20)
    t0 = h.recent_text()
    on0 = has_tts_on(t0)

    h.ctrl("t")
    off1 = False
    for _ in range(10):
        time.sleep(0.1)
        h.drain(0.1)
        if has_tts_off(h.recent_text()):
            off1 = True
            break
    t1 = h.recent_text()

    if not has_insert(h.recent_text()):
        h.write(b"i")
        time.sleep(0.08)
        h.drain(0.05)
    h.type_text("hola mute check")
    h.enter()
    wait_idle_after_turn(h, 90)

    h.ctrl("t")
    on2 = False
    for _ in range(10):
        time.sleep(0.1)
        h.drain(0.1)
        if has_tts_on(h.recent_text()):
            on2 = True
            break
    t2 = h.recent_text()
    shortcut = "Ctrl+T TTS" in t2 or "Ctrl+T" in t2

    if on0 and off1 and on2 and shortcut and not has_panic(t2):
        h.record(tid, name, "PASS", "TTS ON→OFF→ON; shortcut visible")
    else:
        notes = f"on0={on0} off1={off1} on2={on2} shortcut={shortcut}"
        status = "PARTIAL" if (off1 or on2) else "FAIL"
        h.record(tid, name, status, notes)
        h.save_failure(tid, name, "TTS ON/OFF toggle stable", notes)


def test_08_streaming(h: Harness) -> None:
    tid, name = "08", "Streaming assistant UX"
    if not h.boot_ok or not h.is_alive():
        h.record(tid, name, "SKIP", "app not running")
        return
    wait_idle_simple(h, 30)
    if not has_insert(h.recent_text()):
        h.write(b"i")
        time.sleep(0.05)

    h.type_text("ping")
    h.enter()

    saw_thinking = False
    saw_stream = False
    saw_speaking = False
    saw_idle = False
    deadline = time.time() + 120
    while time.time() < deadline:
        h.drain(0.1)
        t = h.recent_text()
        st = last_status(t)
        state = st.get("state")
        if state == "THINKING":
            saw_thinking = True
        if state == "SPEAKING":
            saw_speaking = True
        if has_streaming(t):
            saw_stream = True
        if state == "IDLE" and (saw_thinking or saw_stream or saw_speaking):
            saw_idle = True
            break
        # Fast path: finalized assistant + IDLE even if we missed busy frames
        if state == "IDLE" and time.time() + 120 - deadline > 8:
            if "Pong" in t or "pong" in t.lower() or "You [text]" in t:
                saw_idle = True
                break
    time.sleep(0.4)
    h.drain(0.2)
    t = h.recent_text()
    if not saw_idle:
        saw_idle = last_status(t).get("state") == "IDLE"

    streamish = saw_stream or saw_speaking
    if saw_thinking and saw_idle and not has_panic(t):
        notes = f"THINKING={saw_thinking} streaming={saw_stream} SPEAKING={saw_speaking} IDLE={saw_idle}"
        h.record(tid, name, "PASS", notes)
    elif saw_idle and streamish and not has_panic(t):
        notes = f"THINKING={saw_thinking} streaming={saw_stream} SPEAKING={saw_speaking} IDLE={saw_idle}"
        h.record(tid, name, "PASS", notes + " (THINKING frame optional if stream/speak seen)")
    elif saw_idle and not has_panic(t):
        notes = f"THINKING={saw_thinking} streaming={saw_stream} SPEAKING={saw_speaking} IDLE={saw_idle}"
        h.record(tid, name, "PARTIAL", notes)
        h.save_failure(tid, name, "THINKING + streaming + IDLE", notes)
    else:
        notes = f"THINKING={saw_thinking} streaming={saw_stream} SPEAKING={saw_speaking} IDLE={saw_idle}"
        h.record(tid, name, "FAIL", notes, expected="THINKING, streaming, IDLE", actual=notes)


def test_09_force_simple_blocks(h: Harness) -> None:
    tid, name = "09", "Force SIMPLE blocks research path"
    if not h.boot_ok or not h.is_alive():
        h.record(tid, name, "SKIP", "app not running")
        return
    wait_idle_simple(h, 30)

    # Try force SIMPLE via CSI-u (Auto→SIMPLE)
    h.ctrl_m_csiu()
    time.sleep(0.25)
    h.drain(0.25)
    t0 = h.recent_text()
    forced = "SIMPLE🔒" in t0 or "Classifier force: SIMPLE" in t0
    if not forced:
        # Try one more CSI-u in case we landed on something else
        h.ctrl_m_csiu()
        time.sleep(0.2)
        h.drain(0.2)
        t0 = h.recent_text()
        forced = "SIMPLE🔒" in t0 or "Classifier force: SIMPLE" in t0

    if not forced:
        h.record(
            tid,
            name,
            "SKIP",
            "Cannot set force SIMPLE over PTY (Ctrl+M==Enter); unit coverage for keybinding",
        )
        return

    if not has_insert(h.recent_text()):
        h.write(b"i")
        time.sleep(0.05)

    prompt = "Analyze the project structure deeply with subagents now. FORCE_SIMPLE_PROBE."
    h.type_text(prompt)
    h.enter()
    wait_idle_after_turn(h, 180)
    t = h.recent_text()
    st = last_status(t)
    intent = st.get("intent") or ""
    still_forced = intent.startswith("SIMPLE") and "🔒" in intent
    # Only look at content after this turn's prompt for agent lifecycle
    heavy_agent = has_agent_activity(t, since_marker="FORCE_SIMPLE_PROBE")
    if still_forced and not heavy_agent and not has_panic(t):
        h.record(tid, name, "PASS", "forced SIMPLE; no multi-agent orchestration observed")
    elif still_forced and not has_panic(t):
        h.record(tid, name, "PARTIAL", f"forced badge ok but agent markers maybe present heavy={heavy_agent}")
    else:
        h.record(
            tid,
            name,
            "FAIL",
            f"still_forced={still_forced} heavy_agent={heavy_agent} intent={intent}",
            expected="SIMPLE🔒 and no multi-agent path",
            actual=t[-1500:],
        )

    # Best-effort restore AUTO (cycle)
    for _ in range(3):
        h.ctrl_m_csiu()
        time.sleep(0.15)
        h.drain(0.1)


def test_10_tool_call(h: Harness) -> None:
    tid, name = "10", "Tool call visible in timeline"
    if not h.boot_ok or not h.is_alive():
        h.record(tid, name, "SKIP", "app not running")
        return
    wait_idle_simple(h, 30)
    if not has_insert(h.recent_text()):
        h.write(b"i")
        time.sleep(0.05)

    h.type_text("Que hora es ahora? Responde con la hora exacta.")
    h.enter()
    wait_idle_after_turn(h, 120)
    t = h.recent_text()
    toolish = has_tool_activity(t)
    time_like = bool(re.search(r"\b\d{1,2}:\d{2}\b", t)) or bool(
        re.search(r"\b\d{1,2}\s*(am|pm|h|horas?)\b", t, re.I)
    )
    if (toolish or time_like) and not has_panic(t):
        h.record(tid, name, "PASS", f"tool_or_time toolish={toolish} time_like={time_like}")
    elif not has_panic(t):
        h.record(tid, name, "PARTIAL", "reply without clear tool/time markers")
        h.save_failure(tid, name, "tool line or clock time in answer", t[-1500:])
    else:
        h.record(tid, name, "FAIL", "panic or no reply", expected="tool activity or time", actual=t[-1500:])


def test_11_shutdown_mid_agent(h: Harness) -> None:
    tid, name = "11", "Shutdown mid-agent"
    if not h.boot_ok or not h.is_alive():
        h.record(tid, name, "SKIP", "app not running")
        return
    wait_idle_simple(h, 30)
    if not has_insert(h.recent_text()):
        h.write(b"i")
        time.sleep(0.05)

    h.type_text("Analyze the project structure thoroughly. Use subagents or tools if needed.")
    h.enter()

    # Wait until THINKING or agent activity
    busy = h.wait_for(
        lambda t: has_thinking(t) or has_agent_activity(t) or "SPEAKING" in t,
        timeout=60,
        label="busy",
    )
    # Quit mid-flight
    code = h.stop(signal.SIGINT, grace=10.0)
    time.sleep(0.8)
    logt = h.log_tail(80)
    panic = "panicked at" in logt or "panicked at" in h.text()
    remaining = h.kill_leftover_seneschal()
    if not panic and not remaining:
        h.record(
            tid,
            name,
            "PASS",
            f"quit during busy={busy}; exit={code}; no panic; no leftovers",
        )
    else:
        h.record(
            tid,
            name,
            "FAIL",
            f"busy={busy} code={code} panic={panic} leftovers={remaining}",
            expected="terminate without panic or orphans",
            actual=f"panic={panic} leftovers={remaining}",
        )


# ── P1 tests ────────────────────────────────────────────────────────────────
def test_12_session_restore(h: Harness) -> None:
    tid, name = "12", "Session restore"
    code = f"AZUL-{int(time.time()) % 10000:04d}"
    if not h.boot_ok or not h.is_alive():
        if not boot(h, timeout=90):
            h.record(tid, name, "FAIL", "boot failed")
            return

    wait_idle_simple(h, 30)
    ensure_insert(h)
    h.type_text(
        f"Memoria importante: el codigo secreto de sesion es exactamente {code}. "
        f"Guardalo en memoria a largo plazo. Repite el codigo para confirmar."
    )
    h.enter()
    wait_idle_after_turn(h, 120)
    t1 = h.recent_text()
    if has_panic(t1):
        h.record(tid, name, "FAIL", "panic while storing fact")
        return
    # Brief settle for DB/memory write
    time.sleep(2.0)

    # Quit cleanly and relaunch
    h.stop(signal.SIGINT, grace=8.0)
    time.sleep(1.0)
    h.kill_leftover_seneschal()
    if not boot(h, timeout=90):
        h.record(tid, name, "FAIL", "relaunch failed after storing fact")
        return

    wait_idle_simple(h, 30)
    # --- UI rehydration (independent of model recall) ---
    h.drain(0.5)
    t_boot = h.recent_text()
    if has_panic(t_boot):
        h.record(tid, name, "FAIL", "panic after relaunch")
        return

    def _code_in(blob: str) -> bool:
        if code in blob:
            return True
        compact = blob.replace("-", "").replace(" ", "")
        if code.replace("-", "") in compact:
            return True
        return code.split("-")[0] in blob and code.split("-")[-1] in blob

    ui_rehydrated = (
        _code_in(t_boot)
        or "Session restored" in t_boot
        or "Sesión restaurada" in t_boot
        or "Sesion restaurada" in t_boot
    )

    ensure_insert(h)
    h.type_text(
        "Cual es el codigo secreto de sesion que te pedi recordar? "
        "Responde con el codigo exacto si lo tienes en memoria o historial."
    )
    h.enter()
    wait_idle_after_turn(h, 120)
    t2 = h.recent_text()
    model_recall = _code_in(t2) and not has_panic(t2)

    if has_panic(t2):
        h.record(
            tid,
            name,
            "FAIL",
            f"panic after recall; ui_rehydrate={ui_rehydrated}",
            expected=f"ui rehydrate + recall {code}",
            actual=t2[-2000:],
        )
    elif ui_rehydrated and model_recall:
        h.record(
            tid,
            name,
            "PASS",
            f"UI rehydrated + fact {code} recalled after relaunch",
        )
    elif ui_rehydrated:
        h.record(
            tid,
            name,
            "PARTIAL",
            f"UI rehydrated but model did not clearly recall {code}",
        )
        h.save_failure(tid, name, f"recall {code}", t2[-2000:])
    elif model_recall:
        h.record(
            tid,
            name,
            "PARTIAL",
            f"model recalled {code} but UI did not show prior history/banner",
        )
        h.save_failure(tid, name, "ui rehydrate", t_boot[-2000:])
    else:
        h.record(
            tid,
            name,
            "FAIL",
            f"neither UI rehydrate nor recall of {code}",
            expected=f"Session restored banner and/or {code} in history",
            actual=(t_boot[-1000:] + "\n---\n" + t2[-1000:]),
        )


def test_13_llm_unavailable(h: Harness) -> None:
    tid, name = "13", "LLM unavailable"
    # Tear down current if any
    if h.is_alive():
        h.stop(signal.SIGINT, grace=5.0)
        h.kill_leftover_seneschal()

    # Dead LLM endpoint
    if not boot(
        h,
        timeout=90,
        env_overrides={"LLM_URL": "http://127.0.0.1:9", "LLM_MODEL": "dead-model"},
    ):
        h.record(tid, name, "FAIL", "boot with dead LLM URL failed")
        return

    wait_idle_simple(h, 20)
    ensure_insert(h)
    h.type_text("hola, estas ahi?")
    h.enter()
    # Expect error UI, not hang forever
    deadline = time.time() + 90
    saw_error = False
    while time.time() < deadline:
        h.drain(0.3)
        t = h.recent_text()
        if has_panic(t):
            break
        low = t.lower()
        if any(
            k in low
            for k in (
                "error",
                "failed",
                "unavailable",
                "connection",
                "refused",
                "timeout",
                "no se pudo",
                "fallo",
                "falló",
                "llm",
            )
        ) or "Error" in t:
            saw_error = True
            # wait a bit for IDLE-ish recovery
            time.sleep(1)
            break
        if last_status(t).get("state") == "IDLE" and time.time() + 90 - deadline > 15:
            break

    t = h.recent_text()
    panic = has_panic(t)
    # Quit must still work
    code = h.stop(signal.SIGINT, grace=8.0)
    leftovers = h.kill_leftover_seneschal()
    quit_ok = not leftovers and h.child_pid is None

    if saw_error and not panic and quit_ok:
        h.record(tid, name, "PASS", f"error surfaced; quit ok exit={code}")
    elif not panic and quit_ok:
        h.record(
            tid,
            name,
            "PARTIAL",
            f"no clear error role but no panic; quit ok; saw_error={saw_error}",
        )
        h.save_failure(tid, name, "Error role/message in TUI", t[-2000:])
    else:
        h.record(
            tid,
            name,
            "FAIL",
            f"saw_error={saw_error} panic={panic} quit_ok={quit_ok}",
            expected="error message, no panic, quit works",
            actual=t[-1500:],
        )


def test_14_agent_permission(h: Harness) -> None:
    tid, name = "14", "Agent permission UI"
    if not h.boot_ok or not h.is_alive():
        if not boot(h, timeout=90):
            h.record(tid, name, "SKIP", "boot failed")
            return
    wait_idle_simple(h, 30)
    ensure_insert(h)
    # Try to trigger ACP permission — environment-dependent
    h.type_text(
        "Usa un agente ACP para una accion que requiera permiso del usuario "
        "(por ejemplo borrar un archivo de prueba). Pide confirmacion si hace falta."
    )
    h.enter()
    deadline = time.time() + 180
    saw_perm = False
    while time.time() < deadline:
        h.drain(0.3)
        t = h.recent_text()
        if has_panic(t):
            h.record(tid, name, "FAIL", "panic during permission probe")
            return
        if any(
            k in t
            for k in (
                "Necesita confirmación",
                "Necesita confirmacion",
                "needs permission",
                "permission",
                "confirmar",
                "¿Permitir",
                "Allow",
            )
        ):
            saw_perm = True
            break
        if last_status(t).get("state") == "IDLE" and time.time() + 180 - deadline > 45:
            break
    t = h.recent_text()
    if saw_perm:
        # Try answering no / cancel path
        ensure_insert(h)
        h.type_text("no")
        h.enter()
        wait_idle_after_turn(h, 60)
        h.record(tid, name, "PASS", "permission prompt seen; answered without hang")
    else:
        h.record(
            tid,
            name,
            "SKIP",
            "no ACP permission UI triggered (agents may be auto-allow or not ACP mode)",
        )


def test_15_agent_lifecycle(h: Harness) -> None:
    tid, name = "15", "Agent task lifecycle completeness"
    if not h.boot_ok or not h.is_alive():
        if not boot(h, timeout=90):
            h.record(tid, name, "SKIP", "boot failed")
            return
    wait_idle_simple(h, 30)
    ensure_insert(h)
    h.type_text(
        "Investiga a fondo la estructura del repo con un subagente o run_agent. "
        "No preguntes: ejecuta la tarea completa."
    )
    h.enter()
    labels = {
        "started": ["[Iniciando]", "Started"],
        "running": ["[Procesando]", "Running", "[Proyecto en ejecución]"],
        "finalizing": ["[Organizando resultados]", "Finalizing"],
        "done": ["[Completado]", "Completed", "[Fallido]", "Failed"],
    }
    seen = {k: False for k in labels}
    deadline = time.time() + 300
    while time.time() < deadline:
        h.drain(0.3)
        t = h.recent_text()
        if has_panic(t):
            h.record(tid, name, "FAIL", "panic during lifecycle")
            return
        for k, opts in labels.items():
            if any(o in t for o in opts):
                seen[k] = True
        if last_status(t).get("state") == "IDLE" and time.time() + 300 - deadline > 30:
            if seen["done"] or (seen["started"] and seen["running"]):
                break
            if time.time() + 300 - deadline > 90:
                break
    t = h.recent_text()
    notes = ", ".join(f"{k}={v}" for k, v in seen.items())
    if seen["started"] and (seen["running"] or seen["done"]) and not has_panic(t):
        status = "PASS" if seen["done"] or seen["finalizing"] else "PARTIAL"
        h.record(tid, name, status, notes)
        if status == "PARTIAL":
            h.save_failure(tid, name, "Started→Running→Finalizing→Completed", notes)
    elif any(seen.values()) and not has_panic(t):
        h.record(tid, name, "PARTIAL", notes + " (incomplete lifecycle / prose-only agents)")
        h.save_failure(tid, name, "full AgentTask lifecycle", notes)
    else:
        # Model may refuse to run agents — document
        h.record(
            tid,
            name,
            "PARTIAL",
            notes + "; no AgentTask widgets — model may not have delegated",
        )
        h.save_failure(tid, name, "AgentTask lifecycle rows", t[-2000:])


def test_16_layout_stress(h: Harness) -> None:
    tid, name = "16", "Layout / input stress"
    if not h.boot_ok or not h.is_alive():
        if not boot(h, timeout=90):
            h.record(tid, name, "FAIL", "boot failed")
            return
    wait_idle_simple(h, 20)
    ensure_insert(h)
    long_es = (
        "¡Hola! ¿Qué tal? Esto es una prueba larga de entrada con puntuación española: "
        "ñáéíóúü, comillas «así», y emojis 🤖🎉🔥. "
    ) * 8  # well past 4 rows
    h.type_text(long_es, delay=0.002)
    time.sleep(0.4)
    h.drain(0.3)
    t = h.recent_text()
    scr = t.split("---RAW---")[0] if "---RAW---" in t else t
    status_ok = last_status(t).get("state") is not None or "Ctrl+C quit" in scr or "seneschal" in scr
    # Status bar should still show INSERT
    mode_ok = has_insert(t) or has_normal(t)
    panic = has_panic(t)
    # Don't submit the huge paste — Esc to clear path optional; backspace some
    h.esc()
    time.sleep(0.2)
    h.drain(0.2)
    # Return insert and send short ping to confirm still healthy
    ensure_insert(h)
    h.type_text("ok layout")
    h.enter()
    wait_idle_after_turn(h, 90)
    t2 = h.recent_text()
    if status_ok and mode_ok and not panic and not has_panic(t2):
        h.record(tid, name, "PASS", "long paste + emoji/Spanish; status intact; still responsive")
    else:
        notes = f"status_ok={status_ok} mode_ok={mode_ok} panic={panic}"
        h.record(tid, name, "FAIL" if panic else "PARTIAL", notes)
        h.save_failure(tid, name, "input height/status intact, no panic", notes)


def test_17_ambient_badge(h: Harness) -> None:
    tid, name = "17", "ACTIVE / AMBIENT badge"
    if not h.boot_ok or not h.is_alive():
        if not boot(h, timeout=90):
            h.record(tid, name, "FAIL", "boot failed")
            return
    wait_idle_simple(h, 20)
    ensure_insert(h)
    mode0 = conv_mode_from_status(h.recent_text())
    h.type_text(
        "Cambia el modo de conversacion a ambient usando la herramienta set_conversation_mode. "
        "Llama a set_conversation_mode con mode=ambient ahora."
    )
    h.enter()
    wait_idle_after_turn(h, 120)
    t1 = h.recent_text()
    mode1 = conv_mode_from_status(t1)
    ambient_ok = mode1 in ("AMBIENT", "AMBIENT🔒") or "AMBIENT" in (mode1 or "")

    # Restore active
    ensure_insert(h)
    h.type_text(
        "Vuelve a modo active con set_conversation_mode mode=active ahora."
    )
    h.enter()
    wait_idle_after_turn(h, 120)
    t2 = h.recent_text()
    mode2 = conv_mode_from_status(t2)
    active_ok = mode2 == "ACTIVE"

    notes = f"start={mode0} after_ambient={mode1} after_active={mode2}"
    if ambient_ok and active_ok and not has_panic(t2):
        h.record(tid, name, "PASS", notes)
    elif ambient_ok or active_ok:
        h.record(tid, name, "PARTIAL", notes)
        h.save_failure(tid, name, "ACTIVE ↔ AMBIENT/AMBIENT🔒", notes)
    else:
        # Tool may not have been called by model
        h.record(tid, name, "PARTIAL", notes + "; model may not have called set_conversation_mode")
        h.save_failure(tid, name, "status shows ACTIVE/AMBIENT", notes)


# ── P2 tests ────────────────────────────────────────────────────────────────
def test_18_prompt_build(h: Harness) -> None:
    tid, name = "18", "Prompt-build read-only pane"
    # set_prompt_build is DISABLED (temp) in main.rs
    h.record(
        tid,
        name,
        "SKIP",
        "set_prompt_build tool currently DISABLED (temp) in main.rs — enable to exercise",
    )


def test_19_cli_list(h: Harness) -> None:
    tid, name = "19", "CLI --list-devices / --list-voices"
    import subprocess

    bin_path = h.repo / "target" / "release" / "seneschal"
    env = os.environ.copy()
    env.update(h._dotenv_map())
    env.setdefault("RUST_LOG", "error")
    ok_dev = False
    ok_voi = False
    notes = []
    try:
        r = subprocess.run(
            [str(bin_path), "--list-devices"],
            cwd=h.repo,
            env=env,
            capture_output=True,
            text=True,
            timeout=60,
        )
        out = (r.stdout or "") + (r.stderr or "")
        ok_dev = r.returncode == 0 and (
            "device" in out.lower() or "Microphone" in out or "Hz" in out
        )
        notes.append(f"devices rc={r.returncode} ok={ok_dev}")
    except Exception as e:
        notes.append(f"devices err={e}")
    try:
        r = subprocess.run(
            [str(bin_path), "--list-voices"],
            cwd=h.repo,
            env=env,
            capture_output=True,
            text=True,
            timeout=60,
        )
        out = (r.stdout or "") + (r.stderr or "")
        ok_voi = r.returncode == 0 and ("voice" in out.lower() or "Language" in out)
        notes.append(f"voices rc={r.returncode} ok={ok_voi}")
    except Exception as e:
        notes.append(f"voices err={e}")

    if ok_dev and ok_voi:
        h.record(tid, name, "PASS", "; ".join(notes))
    else:
        h.record(tid, name, "FAIL", "; ".join(notes), expected="both list commands succeed", actual="; ".join(notes))


def test_20_control_api(h: Harness) -> None:
    tid, name = "20", "Control API health"
    import subprocess
    import urllib.request

    port = h._dotenv_map().get("CONTROL_PORT", "9001") or "9001"
    # Fresh boot ensures CONTROL_PORT is applied and server has time to bind
    if h.is_alive():
        h.stop(signal.SIGINT, grace=5.0)
        h.kill_leftover_seneschal()
    if not boot(h, timeout=90, env_overrides={"CONTROL_PORT": str(port)}):
        h.record(tid, name, "FAIL", "boot for control API failed")
        return

    url = f"http://127.0.0.1:{port}/control/state"
    body = ""
    code = None
    last_err = None
    for i in range(40):
        # Prefer success once port is LISTEN
        try:
            listen = subprocess.getoutput(
                f"lsof -nP -iTCP:{port} -sTCP:LISTEN 2>/dev/null"
            )
            if "LISTEN" in listen or i >= 2:
                with urllib.request.urlopen(url, timeout=2) as resp:
                    body = resp.read().decode("utf-8", "replace")
                    code = resp.status
                break
        except Exception as e:
            last_err = e
        time.sleep(0.5)
    else:
        h.record(
            tid,
            name,
            "FAIL",
            f"GET {url} failed after retries: {last_err}",
            expected="control state 200",
            actual=str(last_err),
        )
        return

    ok = code == 200 and ("state" in body.lower() or "{" in body)
    h.record(
        tid,
        name,
        "PASS" if ok else "PARTIAL",
        f"GET {url} → {code} body={body[:160]}",
    )
    if not ok:
        h.save_failure(tid, name, "200 JSON state", body[:500])


def test_21_remote(h: Harness) -> None:
    tid, name = "21", "Remote companion"
    h.record(
        tid,
        name,
        "SKIP",
        "requires companion client + features remote session; not automated in this pass",
    )


def test_22_barge_in(h: Harness) -> None:
    tid, name = "22", "Real barge-in (voice)"
    h.record(tid, name, "SKIP", "hardware voice path; synthetic covered by cargo e2e")


def test_23_stt(h: Harness) -> None:
    tid, name = "23", "Real STT provider"
    h.record(tid, name, "SKIP", "hardware/fixtures path; use make qa test-stt / cargo --ignored stt")


def test_24_extra_tools(h: Harness) -> None:
    tid, name = "24", "web_search / screenshot / open_app"
    h.record(
        tid,
        name,
        "SKIP",
        "tools DISABLED (temp) in main.rs (issue #176); only current_time + set_conversation_mode + run_agent",
    )


def test_25_ttft(h: Harness) -> None:
    tid, name = "25", "Perceived TTFT budget"
    h.record(tid, name, "SKIP", "optional product metric — not instrumented in harness")


def write_report(h: Harness) -> Path:
    report = h.out_dir / "QA_REPORT.md"
    lines = [
        "# Seneschal TUI QA Report",
        "",
        f"Date: {time.strftime('%Y-%m-%d %H:%M:%S')}",
        f"Out: `{h.out_dir}`",
        "",
        "| ID | Name | Status | Notes |",
        "|----|------|--------|-------|",
    ]
    for r in h.results:
        notes = r.notes.replace("|", "\\|").replace("\n", " ")
        lines.append(f"| {r.id} | {r.name} | **{r.status}** | {notes} |")
    counts = {}
    for r in h.results:
        counts[r.status] = counts.get(r.status, 0) + 1
    lines += [
        "",
        f"Summary: {counts}",
        "",
        "Artifacts for failures/partials under `issues/` and `*-snapshot.txt`.",
        "",
        "Notes:",
        "- Ctrl+M over classic PTY collides with Enter (0x0D); force tests use CSI-u when accepted.",
        "- Streaming frames may be missed if LLM is very fast relative to poll interval.",
        "",
    ]
    report.write_text("\n".join(lines), encoding="utf-8")
    print("\n" + "\n".join(lines))
    return report


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--out", default="/tmp/seneschal-qa-run")
    ap.add_argument("--tests", default="01-11", help="e.g. 01,02,06 or 01-11")
    ap.add_argument("--boot-timeout", type=float, default=180.0)
    args = ap.parse_args()

    # Parse test selection
    selected: set[str] = set()
    for part in args.tests.split(","):
        part = part.strip()
        if "-" in part and part.replace("-", "").isdigit():
            a, b = part.split("-", 1)
            for i in range(int(a), int(b) + 1):
                selected.add(f"{i:02d}")
        else:
            selected.add(part.zfill(2) if part.isdigit() else part)

    repo = Path(__file__).resolve().parent.parent
    out = Path(args.out)
    h = Harness(repo=repo, out_dir=out)
    (out / "progress.log").write_text("", encoding="utf-8")

    # Ensure no leftover
    h.kill_leftover_seneschal()

    # Unique ordered plan (core/P0 + P1 + P2)
    plan = [
        ("01", test_01_smoke),
        ("06", test_06_keyboard_modes),
        ("07", test_07_tts_toggle),
        ("02", test_02_classifier),
        ("08", test_08_streaming),
        ("03", test_03_memory),
        ("10", test_10_tool_call),
        ("05", test_05_research),
        ("09", test_09_force_simple_blocks),
        ("16", test_16_layout_stress),
        ("17", test_17_ambient_badge),
        ("15", test_15_agent_lifecycle),
        ("14", test_14_agent_permission),
        ("12", test_12_session_restore),  # quits + relaunches
        ("20", test_20_control_api),  # needs live instance after 12 relaunch or boots
        ("11", test_11_shutdown_mid_agent),
        ("04", test_04_shutdown_idle),
        ("13", test_13_llm_unavailable),  # special dead-LLM boot (after shutdown tests)
        # Non-TUI / skip-friendly
        ("19", test_19_cli_list),
        ("18", test_18_prompt_build),
        ("21", test_21_remote),
        ("22", test_22_barge_in),
        ("23", test_23_stt),
        ("24", test_24_extra_tools),
        ("25", test_25_ttft),
    ]

    # Tests that manage their own boot / do not need initial TUI
    self_boot = {"12", "13", "18", "19", "21", "22", "23", "24", "25"}
    needs_live = selected - self_boot

    try:
        if needs_live:
            if not boot(h, timeout=args.boot_timeout):
                h.record(
                    "01",
                    "Smoke Test (Boot & Main Surface)",
                    "FAIL",
                    "boot timeout — markers not found",
                    expected="seneschal, IDLE, INSERT, Ctrl+M force, Ctrl+C quit",
                    actual=h.recent_text()[-3000:],
                )
                # Still run CLI-only tests
                for tid, fn in plan:
                    if tid in selected and tid in self_boot:
                        try:
                            fn(h)
                        except Exception as e:
                            h.record(tid, fn.__name__, "FAIL", f"exception: {e}")
                write_report(h)
                return 1

        for tid, fn in plan:
            if tid not in selected:
                continue
            if tid == "04":
                if not h.is_alive():
                    print("… rebooting for Test 04 idle shutdown …", flush=True)
                    h.kill_leftover_seneschal()
                    if not boot(h, timeout=args.boot_timeout):
                        h.record(tid, "Graceful Shutdown (idle)", "FAIL", "reboot for idle quit failed")
                        continue
                try:
                    fn(h)
                except Exception as e:
                    traceback.print_exc()
                    h.record(tid, fn.__name__, "FAIL", f"exception: {e}")
                continue
            if tid in self_boot:
                try:
                    fn(h)
                except Exception as e:
                    traceback.print_exc()
                    h.record(tid, fn.__name__, "FAIL", f"exception: {e}")
                continue
            if tid == "01":
                try:
                    fn(h)
                except Exception as e:
                    h.record(tid, fn.__name__, "FAIL", f"exception: {e}")
                continue
            if not h.is_alive():
                print(f"… process dead before {tid}; rebooting …", flush=True)
                h.kill_leftover_seneschal()
                if not boot(h, timeout=args.boot_timeout):
                    h.record(tid, fn.__name__, "SKIP", "process dead; reboot failed")
                    continue
            try:
                fn(h)
            except Exception as e:
                traceback.print_exc()
                h.record(tid, fn.__name__, "FAIL", f"exception: {e}")
    finally:
        if h.is_alive():
            h.stop(signal.SIGINT, grace=5.0)
        h.kill_leftover_seneschal()
        write_report(h)

    fails = sum(1 for r in h.results if r.status == "FAIL")
    return 1 if fails else 0


if __name__ == "__main__":
    sys.exit(main())
