# DevEco Switch LAN Access

> Status: implemented. This document describes the actual interfaces and security boundaries on the `main` branch as of 2026-08-21.

[简体中文](LAN_ACCESS.md) | English

## 1. Feature Scope

DevEco Switch can start an HTTP service inside the app process, listening on `0.0.0.0:12345` by default. Phones, tablets, or computers on the same LAN can use a browser to:

- view projects and sessions;
- create, rename, pin, archive, and delete sessions;
- page through and search messages;
- send messages and start/stop Agent tasks;
- view Todos, costs, and session-modified files;
- handle tool approvals, plan reviews, and Agent questions;
- track streaming messages and task events via SSE;
- read text files inside the project workspace read-only.

LAN routes never expose terminal execution, arbitrary file writes, device control, Provider/API keys, app configuration, or maintenance/cleanup capabilities.

## 2. Architecture

```text
Browser
  ├─ Static native Web UI (HTML/CSS/JS/manifest/icon)
  ├─ REST /api/*
  └─ SSE  /api/events
           │
           ▼
Hyper LAN Server (in-process with Tauri)
  ├─ token auth & failure lockout
  ├─ read_only write-request interception
  ├─ session-domain route whitelist
  └─ reuses Rust commands / SQLite / Tauri events
```

Implementation locations:

- Service: `src-tauri/src/services/lan_server.rs`;
- Desktop IPC: `src-tauri/src/commands/lan.rs`;
- Web UI: `src-tauri/src/services/lan_ui/`;
- Desktop settings page: `src/pages/LanPage.tsx`;
- Database: migrations `042`—`046`.

The Web UI uses embedded native resources and does not reuse the React/Tauri IPC frontend. The reason: browsers only need LAN session functionality, and a standalone implementation keeps the footprint and permission surface minimal.

## 3. Startup and Configuration

The desktop side can:

- start/stop the LAN service;
- change the listening port;
- enable read-only mode;
- create, view, and revoke access tokens;
- view accessible local IPs and the QR code.

When `lan_config.enabled = 1`, the service starts automatically after the app launches. On port conflicts the service tries successive ports; the actual address is whatever the desktop status page returns.

Each device or user should get a separate token so it can be annotated, given an expiry, and revoked independently.

## 4. Authentication

Every `/api/*` request must carry a token:

```http
Authorization: Bearer 123456
```

Since SSE/EventSource cannot conveniently set the Authorization header, query parameters are also supported:

```text
/api/events?token=123456
```

Auth behavior:

- tokens are random 6-digit numbers;
- the database stores salt + SHA-256 hash;
- to re-render the QR code on the local desktop, the current implementation also stores `token_plain`; this means anyone with read access to the local database can obtain LAN tokens;
- tokens support permanent or expiring durations, annotations, individual revocation, and last-used timestamps;
- comparisons use a constant-time helper;
- consecutive failures trigger a global short lockout returning `retry_after`;
- SSE connections are bound to the token hash; revoking or expiring a token disconnects the connection.

LAN uses HTTP rather than HTTPS, so tokens travel over the LAN link. Use it only on trusted networks; for cross-public-network access, route through a user-managed VPN/zero-trust tunnel and prefer firewall source restrictions.

## 5. Read-Only Mode and File Boundaries

With read-only mode enabled, every request other than GET/HEAD and SSE returns `403`, so sessions cannot be created, messages sent, tasks stopped, or approvals processed.

File reading is the only filesystem exception:

- only paths inside the project workspace are allowed;
- reuses `read_project_file`'s path normalization and boundary validation;
- only supported text content is returned;
- per-file limit is 5MB;
- LAN has no write, delete, copy, move, or command interfaces.

The "session-modified files list" only aggregates relative paths from messages without reading files; the read-only file endpoint is only touched when the user explicitly opens a file.

## 6. REST API

### 6.1 Reads

| Method | Path | Description |
|---|---|---|
| GET | `/api/lan/status` | LAN config summary/health check |
| GET | `/api/projects` | Project list |
| GET | `/api/projects/:id/conversations` | Session list, optional `archived`, `keyword` |
| GET | `/api/projects/:id/pending` | Pending approvals/plans/questions for a project |
| GET | `/api/projects/:id/search?q=` | Message search within a project |
| GET | `/api/search?q=` | Cross-project message search |
| GET | `/api/conversations/:id/messages` | Paginated messages, optional `before`, `limit` (max 200) |
| GET | `/api/conversations/:id/todos` | Todos |
| GET | `/api/conversations/:id/cost` | Session cost |
| GET | `/api/conversations/:id/files` | Session-modified file paths |
| GET | `/api/projects/:id/file?path=` | Read-only file inside the project |

### 6.2 Writes

| Method | Path | Description |
|---|---|---|
| POST | `/api/projects/:id/conversations` | Create a session |
| POST | `/api/conversations/:id/messages` | Save a message only |
| POST | `/api/conversations/:id/stream` | Start an Agent, returns `202` immediately |
| POST | `/api/conversations/:id/stop` | Stop a task |
| POST | `/api/approvals/:request_id` | Handle `approval` / `plan` / `ask` |
| POST | `/api/conversations/:id/rename` | Rename |
| POST | `/api/conversations/:id/pin` | Pin/unpin |
| POST | `/api/conversations/:id/archive` | Archive/unarchive |
| POST | `/api/conversations/:id/delete` | Delete a session and converge running tasks |

`stream` does not synchronously wait for the whole Agent task: the service spawns it in the Tauri runtime, returns immediately, and sends progress and final state over SSE.

## 7. SSE

`GET /api/events` establishes an event stream. The service maintains a bounded event buffer, so browsers joining midway or reconnecting can recover the view via message back-fill.

Event sources match the desktop, including chat deltas, tools, approvals, plans, completion, stop, and errors. The web page must bucket events by `conversation_id` — it cannot assume events belong only to the currently open session.

The browser Notification API is only available while the page is open and authorized. Since plain LAN addresses use HTTP, Web Push/Service Worker notifications that require a secure context cannot be relied on.

## 8. Request Limits and Errors

- non-GET request bodies max out at 50MB, to support multimodal image data URLs;
- empty messages, invalid approval types, and illegal paths return `400`;
- missing/invalid/locked tokens return `401`;
- write requests in read-only mode return `403`;
- unregistered routes return `404`;
- database or internal errors return a unified JSON error envelope.

The frontend should not infer status from Chinese error text; prefer HTTP status codes and structured fields.

## 9. Security Checklist

When modifying LAN functionality, verify:

1. the new route strictly belongs to the session domain;
2. `/api` always authenticates first;
3. read-only blocks every side-effecting request;
4. paths reuse project boundary validation instead of manual concatenation;
5. responses cannot contain API keys, system paths, environment variables, or raw sensitive tool output;
6. delete/stop synchronously converge background tasks;
7. SSE disconnects after token revocation/expiry;
8. new request bodies have size limits;
9. unit tests are added for token, lockout, read-only, and routing.

The main risk of the current LAN design is not the web UI but plaintext HTTP tokens and session write permissions. Default deployments should be restricted to trusted LANs, with short-lived, individually revocable tokens per device.
