# Config File

Default configuration values live in `seneschal.pro.toml` (PRO) or `seneschal.dev.toml` (DEV), selected by the `SENECHAL_ENV` environment variable. The file is also embedded into the binary, so a missing local file falls back to the compiled defaults.

Precedence (highest first):
1. Environment variables (existing names unchanged)
2. Explicit config file path (`SENECHAL_CONFIG_FILE`)
3. Environment-specific config file (`seneschal.{env}.toml` in the current directory)
4. Embedded default config

Use `SENECHAL_CONFIG_FILE=/path/to/custom.toml` to load an alternate file. Partial files are merged with embedded defaults, so only changed values need to be specified.

## Migration from single-seneschal.toml

If you have an existing `data/seneschal.db`, manually move it to `data/pro/seneschal.db`:

```bash
mkdir -p data/pro
mv data/seneschal.db data/pro/seneschal.db
mv data/archives data/pro/archives  # if exists
mv data/speaker.emb data/pro/speaker.emb  # if exists
```

Rename your `seneschal.toml` to `seneschal.pro.toml` and update data paths to `data/pro/`.