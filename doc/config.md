# Config File

Default configuration values live in `voicebot.pro.toml` (PRO) or `voicebot.dev.toml` (DEV), selected by the `VOICEBOT_ENV` environment variable. The file is also embedded into the binary, so a missing local file falls back to the compiled defaults.

Precedence (highest first):
1. Environment variables (existing names unchanged)
2. Explicit config file path (`VOICEBOT_CONFIG_FILE`)
3. Environment-specific config file (`voicebot.{env}.toml` in the current directory)
4. Embedded default config

Use `VOICEBOT_CONFIG_FILE=/path/to/custom.toml` to load an alternate file. Partial files are merged with embedded defaults, so only changed values need to be specified.

## Migration from single-voicebot.toml

If you have an existing `data/voicebot.db`, manually move it to `data/pro/voicebot.db`:

```bash
mkdir -p data/pro
mv data/voicebot.db data/pro/voicebot.db
mv data/archives data/pro/archives  # if exists
mv data/speaker.emb data/pro/speaker.emb  # if exists
```

Rename your `voicebot.toml` to `voicebot.pro.toml` and update data paths to `data/pro/`.