# yzz-llm-proxy

An OpenAI-compatible API proxy that lets you use enterprise LLM providers (Kiro, DashScope, etc.) through a unified `/v1/chat/completions` endpoint.

## Supported Providers

| Provider | Prerequisites |
|----------|---------------|
| Kiro (Enterprise) | Must install and login with [kiro-cli](https://kiro.dev) |
| DashScope  | API key required |

### Kiro Setup

1. Install kiro-cli and complete the login flow:
   ```bash
   # The proxy reads credentials from kiro-cli's local database.
   # You must be logged in before starting the proxy.
   kiro login
   ```
2. Enable the Kiro provider in `config.toml`:
   ```toml
   [[accounts]]
   provider = "kiro"
   ```

## Configuration

On first run, the proxy creates a default `config.toml` at:

- **Windows**: `%USERPROFILE%\.config\yzz-llm-proxy\config.toml`
- **Linux/macOS**: `~/.config/yzz-llm-proxy/config.toml`

Example:
```toml
[server]
listen = "127.0.0.1:8127"

[[accounts]]
provider = "kiro"
```

## Integration with Coding Agents

The proxy exposes an OpenAI-compatible API, so any tool that supports a custom OpenAI base URL can use it.

### OpenCode

Add to your `opencode.json`:
- **Windows**: `%USERPROFILE%\.config\opencode\opencode.json`
- **Linux/macOS**: `~/.config/opencode/opencode.json`
```json
{
    "$schema": "https://opencode.ai/config.json",
    "provider": {
        ...
        "llm-api-proxy": {
            "npm": "@ai-sdk/openai-compatible",
            "name": "LLM API Proxy",
            "options": {
                "baseURL": "http://127.0.0.1:8127/v1",
                "apiKey": "dummy"
            },
            "models": {
                "kiro/glm-5": {
                    "name": "Kiro GLM 5",
                    "modalities": {
                        "input": ["text"],
                        "output": ["text"]
                    },
                    "limit": {
                        "context": 200000,
                        "output": 65536
                    }
                }
            }
        },
```

### Claude Code

Claude Code supports OpenAI-compatible providers via `--provider` flag or environment variables:
```bash
export OPENAI_API_KEY="dummy"
export OPENAI_BASE_URL="http://127.0.0.1:8127/v1"
claude --provider openai --model "kiro/glm-5"
```

### General (any OpenAI-compatible client)

Point the client to:
- **Base URL**: `http://127.0.0.1:8127/v1`
- **API Key**: any non-empty string (the proxy handles auth internally)
- **Model**: `kiro/<model-id>` (e.g. `kiro/glm-5`)

## API Endpoints

- `POST /v1/chat/completions` — Chat completions (streaming and non-streaming)
- `GET /v1/models` — List available models