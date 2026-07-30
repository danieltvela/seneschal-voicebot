# Add a new type of greeting (no-device startup)

## Context
- Origin: Gitea issue #169 — Add a new type of greeting
- Summary: When the app launches with no audio capture device available (`capture_stream.is_none()`), send a simpler greeting notification to the LLM immediately, instead of deferring entirely until a device connects. The existing `DeviceConnected` handler still sends the full startup greeting when a device eventually becomes available.
- Proposed branch: feature/issue-169-add-a-new-type-of-greeting
- Base branch: master
- Assumptions:
  - The new "no_device_startup" notification is sent regardless of `is_first_launch` (no differentiation needed).
  - The existing `DeviceConnected` handler behavior is unchanged — it still sends the full `first_launch`/`startup` notification when the device connects later. This means the LLM will receive two notifications in the no-device scenario (a brief one at launch, a fuller one on device connect), which is intentional two-phase behavior.
  - The notification must be sent via `transcript_tx.send(PipelineFrame::SystemNotification { ... })`, which is the same mechanism used for all other startup greetings.

## Phase 1: Add i18n key for "no_device_startup"

- [x] Step 1.1: Add the `no_device_startup` notification entries in the i18n module
  - File(s): `crates/seneschal-common/src/i18n.rs`
  - Change: Insert the following two match arms **after** the existing `("startup", "en")` arm (currently around line 24) and **before** the `("background_task_done", "es")` arm (currently around line 27):
    ```rust
    ("no_device_startup", "es") => {
        "[Sistema: seneschal acaba de arrancar.]"
    }
    ("no_device_startup", "en") => {
        "[System: seneschal just started.]"
    }
    ```
  - The exact strings are:
    - ES: `"[Sistema: seneschal acaba de arrancar.]"`
    - EN: `"[System: seneschal just started.]"`
  - Acceptance criteria: The file compiles (`cargo check` passes with no errors). The new key returns the correct string when called as `seneschal_common::i18n::get_notification("no_device_startup", "es")`.

## Phase 2: Send the no-device greeting at startup in main.rs

- [x] Step 2.1: Add an `else` branch to the startup greeting block to handle `capture_stream.is_none()`
  - File(s): `src/main.rs`
  - Change: In the `// ── Startup greeting / first-time introduction ─────────────────────────────` block (currently lines 1314–1335), the existing code is:
    ```rust
    if capture_stream.is_some() {
        let first = is_first_launch;
        let key = if first { "first_launch" } else { "startup" };
        let notification = seneschal_common::i18n::get_notification(key, &config.language);
        let notification = if first {
            notification.to_string()
        } else {
            let now = chrono::Local::now();
            let time_str = now.format("%H:%M").to_string();
            let date_str = now.format("%d/%m/%Y").to_string();
            notification
                .replace("{time_str}", &time_str)
                .replace("{date_str}", &date_str)
        };
        transcript_tx
            .send(PipelineFrame::SystemNotification { text: notification })
            .await
            .ok();
    }
    ```
    Add an `else` branch immediately after the closing `}` of the `if` block (i.e., after `transcript_tx.send(...).await.ok();` and the closing `}`), so the full block becomes:
    ```rust
    if capture_stream.is_some() {
        // ... existing code unchanged ...
    } else {
        let msg = seneschal_common::i18n::get_notification("no_device_startup", &config.language);
        transcript_tx
            .send(PipelineFrame::SystemNotification {
                text: msg.to_string(),
            })
            .await
            .ok();
    }
    ```
    - Note: The `else` branch does NOT need `is_first_launch` differentiation, does NOT need time/date substitution.
  - Acceptance criteria: `cargo build` succeeds. When the app launches without an audio input device, the transcript channel receives a `SystemNotification` with the `no_device_startup` text. When an audio input device IS available at launch, the existing behavior (first_launch/startup greeting) is unchanged.

## Phase 3: QA validation

- [x] Step 3.1: Run the QA suite
  - Commands (in order):
    1. `cargo fmt --check`
    2. `cargo clippy --all-targets --no-deps -- -D warnings`
    3. `cargo test`
    4. `cargo test --features full`
    5. `cargo build --features full`
  - Acceptance criteria: All commands pass with exit code 0, no warnings, no test failures.
