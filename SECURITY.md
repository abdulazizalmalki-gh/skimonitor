# Security policy

## Supported versions

Security fixes land on the latest release (`0.3.x` at the time of writing) and on `master`.
Older tags are not patched — upgrade to the newest release archive.

## Reporting a vulnerability

Please report privately through GitHub:

**https://github.com/abdulazizalmalki-gh/skimonitor/security/advisories/new**

(Repository → *Security* → *Advisories* → *Report a vulnerability*.) That opens a private
thread visible only to the maintainer; a public issue would disclose the problem before a fix
exists.

Useful in a report: version (`skimonitor --help` prints it in the header of the UI, or
`cargo metadata`), the mode (`TUI`, `--web`, `--once`), a reproducer, and what you think an
attacker gains. If you need to attach metrics or screenshots, remember they may contain host
names and process command lines — redact them.

Expect an acknowledgement within a few days. Fixes are shipped as a release plus a note in the
changelog; you will be credited unless you ask not to be.

## Scope

In scope — the parts this project actually owns:

- the `--web` HTTP/SSE server (`src/server.rs`): request parsing, the write endpoints, the
  origin check, header handling, path traversal, event-stream handling;
- the probe (`probe.sh`) and how its output is parsed (`src/model.rs`, `src/engine.rs`): command
  and JSON injection from a *host* that returns hostile data;
- how targets become `ssh` argv (`src/ssh.rs`, `src/app.rs`): option injection, a target that
  escapes the intended host, credential or key handling;
- the browser page (`web/`): XSS through host names, process command lines or model names.

Out of scope — by design, and documented in the README's Security section:

- an exposed `--web` port without authentication. Anyone who can reach it is an operator: they
  can read every card (including process command lines) and add, drop or restart hosts. Bind it
  to loopback behind an authenticating proxy, or keep it on a trusted network. Reports that
  amount to "the port is reachable and unauthenticated" are not vulnerabilities;
- an attacker who already has your user account, your `ssh-agent`, or your keys;
- what a host you asked skimonitor to monitor can read about itself — the probe runs as your
  SSH user on that machine;
- secrets placed in a process's `argv`: skimonitor shows command lines verbatim, exactly like
  `ps`.

## What the project already guarantees

- Every `ssh` invocation puts `--` ahead of the target, and option-shaped targets are refused
  when a host is added (CLI, add prompt, HTTP endpoint alike).
- Write endpoints refuse browser requests whose `Origin` is not the served page (`403`), so a
  third-party site cannot drive the dashboard.
- The page is served with `default-src 'none'` CSP, `nosniff`, `no-referrer`, and no external
  resources of any kind — the JS, CSS and font are embedded in the binary.
- No metrics are written to disk, and there is no telemetry or update check.
