# PLAN 19 — Update Service and Freminal Website

## Overview

This plan describes a **separate repository** (`freminal-updates`) that hosts two things:

1. **Update Service** — A Cloudflare Worker at `updates.freminal.dev` that acts as a caching
   proxy for the GitHub Releases API, providing a stable `/v1/latest.json` endpoint that the
   Freminal terminal emulator queries to check for new versions.
2. **Project Website** — A static site at `freminal.dev` with download links, feature overview,
   installation instructions, and documentation.

Both are deployed via Cloudflare (Worker for the API, Pages for the website).

> **Context for agents working in this repo:** You will NOT have access to the Freminal
> terminal emulator source code. This document provides all the information you need about
> Freminal's architecture, release process, and API contract. Do not make assumptions about
> the Freminal codebase beyond what is documented here.

---

## Context: What Is Freminal?

Freminal is a modern GPU-accelerated terminal emulator written in Rust. It is open-source
(MIT license) and hosted at `https://github.com/fredsystems/freminal`.

### Release Process

Freminal releases are triggered by pushing a `v*` tag to the GitHub repository. A GitHub Actions
workflow builds binaries for four platforms and creates a GitHub Release with the following assets:

| Asset Pattern                         | Platform            | Format            |
| ------------------------------------- | ------------------- | ----------------- |
| `freminal-{VER}-linux-amd64.tar.gz`   | Linux x86_64        | Compressed binary |
| `freminal-{VER}-linux-amd64.deb`      | Linux x86_64        | Debian package    |
| `freminal-{VER}-linux-amd64.AppImage` | Linux x86_64        | AppImage          |
| `freminal-{VER}-linux-arm64.tar.gz`   | Linux ARM64         | Compressed binary |
| `freminal-{VER}-linux-arm64.deb`      | Linux ARM64         | Debian package    |
| `freminal-{VER}-linux-arm64.AppImage` | Linux ARM64         | AppImage          |
| `freminal-{VER}-macos-arm64.tar.gz`   | macOS Apple Silicon | Compressed binary |
| `freminal-{VER}-macos-arm64.app.zip`  | macOS Apple Silicon | macOS .app bundle |
| `freminal-{VER}-windows-amd64.zip`    | Windows x86_64      | Compressed exe    |
| `SHA256SUMS`                          | All                 | Checksums file    |

- Version format is SemVer: `MAJOR.MINOR.PATCH` (e.g., `0.1.4`, `0.2.0`).
- Tags are `v` prefixed: `v0.1.4`, `v0.2.0`.
- The GitHub repository is `fredsystems/freminal`.
- Releases use GitHub's auto-generated release notes.

### Client-Side Update Check (What Freminal Expects)

The Freminal binary makes a single HTTP GET request on startup (at most once every 24 hours,
cached locally):

```text
GET https://updates.freminal.dev/v1/latest.json
```

With headers:

```text
User-Agent: freminal/{current_version}
Accept: application/json
```

Timeout: 5 seconds. The check runs on a background thread. If it fails (network error, timeout,
non-200 response), it is silently ignored — the terminal continues to function normally.

The client expects the response format documented in the API section below.

---

## Part 1: Update Service (Cloudflare Worker)

### Architecture

```text
Freminal Client                    Cloudflare Worker                  GitHub API
  GET /v1/latest.json ───────────► check KV cache ──── hit ──► return cached response
                                        │
                                   miss/expired
                                        │
                                        ▼
                                   GET github.com/repos/fredsystems/freminal/releases/latest
                                        │
                                        ▼
                                   Transform response
                                   Store in KV (TTL: 1 hour)
                                        │
                                        ▼
                                   Return to client
```

Cache invalidation flow (triggered by Freminal's deploy workflow):

```text
GitHub Actions                     Cloudflare Worker
  POST /v1/invalidate ────────────► verify bearer token
  { "tag": "v0.2.0" }                   │
                                   delete KV cache entry
                                        │
                                   return 200 OK
```

### Technology Stack

- **Runtime:** Cloudflare Workers (JavaScript/TypeScript)
- **Cache:** Cloudflare Workers KV
- **Language:** TypeScript
- **Build tool:** Wrangler (Cloudflare CLI)
- **Testing:** Vitest (unit tests for transform logic, integration tests with miniflare)

### API Endpoints

#### `GET /v1/latest.json`

Returns information about the latest Freminal release.

**Response (200 OK):**

```json
{
  "version": "0.2.0",
  "tag": "v0.2.0",
  "published_at": "2026-03-29T12:00:00Z",
  "download_url": "https://github.com/fredsystems/freminal/releases/tag/v0.2.0",
  "release_notes_url": "https://github.com/fredsystems/freminal/releases/tag/v0.2.0",
  "assets": [
    {
      "name": "freminal-0.2.0-linux-amd64.tar.gz",
      "url": "https://github.com/fredsystems/freminal/releases/download/v0.2.0/freminal-0.2.0-linux-amd64.tar.gz",
      "size": 12345678,
      "sha256": "abc123def456..."
    },
    {
      "name": "freminal-0.2.0-linux-amd64.deb",
      "url": "https://github.com/fredsystems/freminal/releases/download/v0.2.0/freminal-0.2.0-linux-amd64.deb",
      "size": 9876543,
      "sha256": "789xyz..."
    }
  ],
  "sha256sums_url": "https://github.com/fredsystems/freminal/releases/download/v0.2.0/SHA256SUMS"
}
```

**Response headers:**

- `Content-Type: application/json`
- `Cache-Control: public, max-age=3600` (1 hour)
- `Access-Control-Allow-Origin: *` (CORS — allows browser-based checks from the website)

**Error responses:**

- `502 Bad Gateway` — GitHub API unavailable or returned an error
- `500 Internal Server Error` — unexpected worker error

The `assets` array includes every asset from the GitHub release. Each asset includes:

- `name`: the filename
- `url`: direct download URL from GitHub
- `size`: file size in bytes
- `sha256`: SHA256 hash (extracted from the `SHA256SUMS` release asset if available; `null` if
  not available)

**SHA256 extraction logic:** When transforming the GitHub release data, the worker fetches the
`SHA256SUMS` asset (if present), parses it (format: `<hash>  <filename>\n` per line), and
populates the `sha256` field for each matching asset. If `SHA256SUMS` is not present or cannot
be parsed, `sha256` is `null` for all assets.

#### `POST /v1/invalidate`

Cache invalidation endpoint called by the Freminal deploy workflow after a new release is
created.

**Request:**

```text
Authorization: Bearer <token>
Content-Type: application/json

{
  "tag": "v0.2.0"
}
```

**Response (200 OK):**

```json
{
  "ok": true,
  "message": "Cache invalidated"
}
```

**Error responses:**

- `401 Unauthorized` — missing or invalid bearer token
- `400 Bad Request` — missing `tag` field

The bearer token is stored as a Cloudflare Worker secret (`INVALIDATE_TOKEN`). It must match
the `UPDATE_SERVICE_TOKEN` GitHub Actions secret in the Freminal repository.

#### `GET /health`

Health check endpoint.

**Response (200 OK):**

```json
{
  "ok": true,
  "service": "freminal-updates",
  "timestamp": "2026-03-29T12:00:00Z"
}
```

### KV Schema

KV namespace: `FREMINAL_UPDATES`

| Key              | Value                                                   | TTL            |
| ---------------- | ------------------------------------------------------- | -------------- |
| `latest_release` | JSON string (same format as `/v1/latest.json` response) | 3600s (1 hour) |

### GitHub API Interaction

The worker calls the GitHub REST API:

```text
GET https://api.github.com/repos/fredsystems/freminal/releases/latest
```

With headers:

```text
Accept: application/vnd.github.v3+json
User-Agent: freminal-updates-worker
```

**No authentication required** — the Freminal repository is public. GitHub's unauthenticated
rate limit is 60 requests/hour per IP. Since the worker caches for 1 hour and only the
invalidation endpoint triggers a fresh fetch, this is more than sufficient.

If rate limiting becomes an issue in the future, a `GITHUB_TOKEN` secret can be added to the
worker configuration to increase the limit to 5000/hour.

### Transform Logic

The worker transforms the GitHub API response into the `/v1/latest.json` format:

1. Extract `tag_name` → `tag`, strip `v` prefix → `version`.
2. Extract `published_at`.
3. Build `download_url` and `release_notes_url` from the tag.
4. Map `assets[]` → filter to only files matching known Freminal asset patterns, extract
   `name`, `browser_download_url` → `url`, `size`.
5. Fetch `SHA256SUMS` asset (if present in the release), parse it, and populate `sha256`
   fields.
6. Build `sha256sums_url` from the SHA256SUMS asset URL (if present).

### Wrangler Configuration

```toml
# wrangler.toml
name = "freminal-updates"
main = "src/index.ts"
compatibility_date = "2024-01-01"

[vars]
GITHUB_REPO = "fredsystems/freminal"

[[kv_namespaces]]
binding = "FREMINAL_UPDATES"
id = "<kv-namespace-id>"

# Secrets (set via `wrangler secret put`):
# INVALIDATE_TOKEN — bearer token for /v1/invalidate
```

### Project Structure

```text
freminal-updates/
├── src/
│   ├── index.ts          # Worker entry point, router
│   ├── github.ts         # GitHub API client, response transform
│   ├── cache.ts          # KV cache read/write helpers
│   ├── sha256.ts         # SHA256SUMS parser
│   └── types.ts          # TypeScript interfaces
├── test/
│   ├── transform.test.ts # Unit tests for GitHub → API response transform
│   ├── sha256.test.ts    # Unit tests for SHA256SUMS parsing
│   ├── router.test.ts    # Integration tests with miniflare
│   └── fixtures/         # Sample GitHub API responses
├── site/                 # Website (see Part 2)
├── wrangler.toml
├── package.json
├── tsconfig.json
├── vitest.config.ts
├── .github/
│   └── workflows/
│       └── deploy.yml    # Wrangler deploy + site deploy
└── README.md
```

---

## Part 2: Project Website

### Scope

A static website at `freminal.dev` serving as the project's public face. Deployed via
Cloudflare Pages from the `site/` directory in the same repo.

### Content Pages

1. **Home / Landing** — Hero section with tagline ("A modern terminal emulator built in Rust"),
   screenshot, key features (GPU-accelerated, ligatures, image support, 25+ themes), and
   download CTA button.

2. **Download** — Platform-specific download links. Pulls latest version info from
   `https://updates.freminal.dev/v1/latest.json` via client-side JavaScript to always show
   current version and direct download links. Includes:
   - Linux: .deb, .AppImage, .tar.gz (amd64 and arm64)
   - macOS: .app.zip, .tar.gz (arm64)
   - Windows: .zip (amd64)
   - Nix: flake install instructions
   - Auto-detect platform and highlight the recommended download.

3. **Features** — Detailed feature descriptions:
   - Custom GPU renderer (OpenGL shaders via egui PaintCallback)
   - Font ligature support (OpenType liga, calt, dlig)
   - 25+ built-in color themes (Catppuccin, Dracula, Nord, Solarized, etc.)
   - Inline image display (iTerm2 and Kitty protocols)
   - TOML configuration with hot-reload
   - Nix/home-manager integration
   - Recording & playback
   - Sub-millisecond frame times

4. **Installation** — Detailed install instructions per platform:
   - Linux: .deb, AppImage, raw binary, Nix flake
   - macOS: .app bundle, raw binary, Nix flake
   - Windows: exe, manual PATH setup
   - Nix: flake + home-manager module configuration example

5. **Configuration** — Documents the TOML config format. Sections: font, cursor, theme, shell,
   logging, scrollback, UI. Example config file. Home-manager option reference.

### Technology

- **Static site generator:** Choose one of: Astro, Hugo, or plain HTML/CSS/JS. Recommendation:
  **Astro** — good DX, generates static HTML, supports MDX for documentation pages, TypeScript
  native.
- **Styling:** Tailwind CSS
- **Deployment:** Cloudflare Pages (connected to the `site/` directory)
- **Dynamic version info:** Small client-side script fetches `/v1/latest.json` on the download
  page to display current version and direct links.

### Design Notes

- Dark theme by default (matching the terminal aesthetic).
- Responsive (mobile-friendly, but primary audience is desktop developers).
- Fast: minimal JavaScript, static HTML, edge-cached by Cloudflare.
- Use the Freminal app icon (`assets/icon.png` in the main repo) as favicon/logo. A copy
  should be committed to this repo under `site/public/`.

---

## Part 3: Deployment & CI

### GitHub Actions Workflow

`.github/workflows/deploy.yml`:

```yaml
name: Deploy

on:
  push:
    branches: [main]
  pull_request:
    branches: [main]

jobs:
  test:
    name: Test Worker
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: actions/setup-node@v4
        with:
          node-version: "22"
      - run: npm ci
      - run: npm test

  deploy-worker:
    name: Deploy Worker
    needs: test
    if: github.ref == 'refs/heads/main'
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: actions/setup-node@v4
        with:
          node-version: "22"
      - run: npm ci
      - run: npx wrangler deploy
        env:
          CLOUDFLARE_API_TOKEN: ${{ secrets.CLOUDFLARE_API_TOKEN }}

  deploy-site:
    name: Deploy Website
    needs: test
    if: github.ref == 'refs/heads/main'
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: actions/setup-node@v4
        with:
          node-version: "22"
      - run: npm ci
      - working-directory: site
        run: npm ci && npm run build
      - uses: cloudflare/pages-action@v1
        with:
          apiToken: ${{ secrets.CLOUDFLARE_API_TOKEN }}
          accountId: ${{ secrets.CLOUDFLARE_ACCOUNT_ID }}
          projectName: freminal-site
          directory: site/dist
```

### DNS Configuration

| Record                 | Type  | Value                                    | Notes                |
| ---------------------- | ----- | ---------------------------------------- | -------------------- |
| `updates.freminal.dev` | CNAME | `freminal-updates.<account>.workers.dev` | Worker custom domain |
| `freminal.dev`         | CNAME | `freminal-site.pages.dev`                | Pages custom domain  |
| `www.freminal.dev`     | CNAME | `freminal.dev`                           | Redirect             |

### Secrets Required

| Secret                  | Where                          | Purpose                                  |
| ----------------------- | ------------------------------ | ---------------------------------------- |
| `CLOUDFLARE_API_TOKEN`  | GitHub Actions                 | Deploy worker + pages                    |
| `CLOUDFLARE_ACCOUNT_ID` | GitHub Actions                 | Pages deployment                         |
| `INVALIDATE_TOKEN`      | Cloudflare Worker              | Verify invalidation requests             |
| `UPDATE_SERVICE_TOKEN`  | Freminal repo (GitHub Actions) | Sent as bearer token to `/v1/invalidate` |

`INVALIDATE_TOKEN` (worker side) and `UPDATE_SERVICE_TOKEN` (Freminal repo side) must be the
same value.

---

## Subtasks

### Worker

- [ ] **19.1** Initialize repo and project scaffolding
  - Create `freminal-updates` repo.
  - Initialize npm project with TypeScript, Wrangler, Vitest.
  - Create `wrangler.toml` with KV namespace binding.
  - Create `tsconfig.json`, `vitest.config.ts`, `.gitignore`.
  - Create `src/types.ts` with all TypeScript interfaces.
  - **Verify:** `npm run build` succeeds. `wrangler dev` starts locally.

- [ ] **19.2** Implement GitHub API client and transform logic
  - Create `src/github.ts`: `fetchLatestRelease()` function that calls GitHub API, transforms
    the response into the `/v1/latest.json` format.
  - Create `src/sha256.ts`: parser for `SHA256SUMS` file format.
  - Create test fixtures in `test/fixtures/` with sample GitHub API responses.
  - Add unit tests in `test/transform.test.ts` and `test/sha256.test.ts`.
  - **Verify:** `npm test` passes.

- [ ] **19.3** Implement KV caching layer
  - Create `src/cache.ts`: `getCachedRelease()`, `setCachedRelease()`, `invalidateCache()`.
  - KV key: `latest_release`, TTL: 3600 seconds.
  - Add integration tests with miniflare.
  - **Verify:** `npm test` passes.

- [ ] **19.4** Implement Worker router and endpoints
  - Create `src/index.ts`: request router.
  - `GET /v1/latest.json` — check cache, fetch if miss, return response.
  - `POST /v1/invalidate` — verify bearer token, delete cache, return 200.
  - `GET /health` — return health check response.
  - CORS headers on all responses.
  - Add integration tests in `test/router.test.ts`.
  - **Verify:** `npm test` passes. `wrangler dev` serves correct responses locally.

- [ ] **19.5** Deploy Worker to Cloudflare
  - Create KV namespace via Wrangler CLI.
  - Set `INVALIDATE_TOKEN` secret via `wrangler secret put`.
  - Configure custom domain `updates.freminal.dev`.
  - Create GitHub Actions workflow (`.github/workflows/deploy.yml`).
  - Add `CLOUDFLARE_API_TOKEN` secret to the repo.
  - **Verify:** `https://updates.freminal.dev/health` returns 200.
    `https://updates.freminal.dev/v1/latest.json` returns latest Freminal release data.

### Website

- [ ] **19.6** Initialize website project
  - Create `site/` directory with Astro project scaffolding.
  - Install Tailwind CSS.
  - Create base layout (dark theme, responsive, header/footer).
  - Add Freminal icon as favicon.
  - **Verify:** `npm run build` in `site/` succeeds. `npm run dev` serves locally.

- [ ] **19.7** Build Home / Features page
  - Landing page with hero section, screenshot, feature list.
  - Features page with detailed descriptions.
  - **Verify:** Pages render correctly in browser.

- [ ] **19.8** Build Download page
  - Platform-specific download sections.
  - Client-side JS that fetches `/v1/latest.json` to show current version and direct links.
  - Platform auto-detection to highlight recommended download.
  - Nix flake install instructions.
  - **Verify:** Download links point to real release assets (after at least one release).

- [ ] **19.9** Build Installation and Configuration pages
  - Detailed per-platform installation instructions.
  - TOML config reference (all sections: font, cursor, theme, shell, logging, scrollback, UI).
  - Home-manager module usage example.
  - **Verify:** Pages render correctly.

- [ ] **19.10** Deploy website to Cloudflare Pages
  - Connect Cloudflare Pages to the repo's `site/` directory.
  - Configure custom domain `freminal.dev`.
  - Add deploy job to GitHub Actions workflow.
  - **Verify:** `https://freminal.dev` is live and serves the site.

---

## Verification

### Worker Verification

- All unit and integration tests pass (`npm test`)
- `GET /v1/latest.json` returns valid JSON matching the documented schema
- `POST /v1/invalidate` with valid token returns 200 and clears cache
- `POST /v1/invalidate` with invalid token returns 401
- `GET /health` returns 200
- CORS headers are present on all responses
- Response time < 100ms for cache hits
- Cache correctly expires after 1 hour

### Website Verification

- All pages render correctly on desktop and mobile
- Download page dynamically loads latest version from the API
- Platform detection highlights correct download
- Lighthouse score > 90 for performance
- Site loads in < 2 seconds

### End-to-End

- Freminal binary can successfully query `https://updates.freminal.dev/v1/latest.json`
- After a new Freminal release, the deploy workflow invalidation webhook clears the cache
- The next client query after invalidation returns the new version
