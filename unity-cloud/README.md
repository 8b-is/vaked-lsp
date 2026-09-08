# unity-cloud — the Unity Cloud Asset Manager lane

The Unity Cloud Python SDK, wired into the constellation as a uv lane and an
MCP tool. One door, many lanes: the same SDK surface, scripted for the
engine's asset pipeline.

## install

The SDK ships from Unity's private index (not public PyPI):

```bash
cd unity-cloud
uv add --index https://unity3ddist.jfrog.io/artifactory/api/pypi/am-pypi-prod-local/simple \
  --index-strategy unsafe-best-match "unity-cloud==0.10.11"
```

## auth

Two modes:

- **user_login** (default) — browser OAuth PKCE. Run `auth` once; the SDK
  keeps the session.
- **service_account** — headless, for CI/agents:

```bash
export UNITY_CLOUD_KEY_ID=... UNITY_CLOUD_KEY=...
uv run python cli.py --service-account whoami
```

## usage

```bash
uv run python cli.py auth                     # browser sign-in
uv run python cli.py projects                 # orgs + projects
uv run python cli.py assets --org O --project P
uv run python cli.py search --org O --project P --limit 20
uv run python cli.py datasets --org O --project P --asset A --version V
uv run python cli.py upload   --org O --project P --asset A --version V --dataset D --file ./mesh.fbx
uv run python cli.py download --org O --project P --asset A --version V --dataset D --file mesh.fbx --out ./
```

## MCP wiring

`vaked-mcp` (the umbrella sidecar) exposes `unity_cloud` — it dispatches to
this lane:

```json
{ "name": "unity_cloud", "arguments": { "action": "assets", "org": "...", "project": "..." } }
```

The lane stays dark when the uv project is missing — the umbrella doctrine.

## scope

The SDK covers Unity Cloud **authorization + Asset Manager** (assets,
versions, datasets, files, metadata, collections, transformations, VCS
integrations). Cloud Build and other Unity Gaming Services are not in this
SDK.

— the constellation · 0 + 1 · fine touch from within · vaked.dev
