# 005 — Authentication

Status: implemented
Updated: 2026-10-16

## Design

### Admin password
- `rustybee set-password` reads the password twice from the TTY, or once from stdin with `--stdin`. It needs at least 10 characters and stores an Argon2id hash with default params in `settings.admin_password`.
- On `serve`, if no password is set and `RUSTYBEE_ADMIN_PASSWORD` is present, the server hashes and stores it once. If neither exists, the UI shows "run `rustybee set-password`" and login is disabled.

### Login methods
- The default is a one-time code texted to the admin phone through a connected device. The password is the fallback.
- **Admin phone:** `settings.admin_phone`, set on the Settings page. A code is texted to the new number and must be entered there before the number is stored. It can be removed.
- A **code** can be requested when an admin phone is set and at least one enabled device is connected.
- The **password** is accepted only when a password is set and at least one of these holds:
  - no admin phone is set (bootstrap);
  - no device is connected;
  - `settings.otp_send_failures` ≥ `RUSTYBEE_OTP_MAX_FAILURES` (default 3, minimum 1).
- `POST /login` re-checks this and returns 403 when the password is not allowed. `rustybee set-password` stays the recovery path.

### Login codes
- `POST /login/code`:
  - generates 6 random digits valid for 5 minutes, keeps only their SHA-256 in memory, and replaces any earlier pending code;
  - queues `rustybee login code: 123456. Expires in 5 minutes.` to the admin phone on the default device if it is connected, otherwise on any connected device; the message is marked `internal`;
  - is limited to one send per 30 s, 5 per 15 min per client IP, and 20 per hour overall. Over a limit it returns 429; with no phone or no connected device it returns 409.
- `POST /login/verify`: constant-time compare; 5 wrong tries end the code. A right code creates a session (303), and the code can't be reused.
- **Failure counter:** `otp_send_failures` goes up by one when a code message ends `failed` or `expired`, disappears, or is still not `sent` after 90 s. Each message counts once. It resets to 0 when a code message is `sent` or `delivered`, and on any successful login.
- After a code message finishes, its body is replaced with "Login code (hidden)".
- Pending codes live in memory, so a restart ends them.

### Sessions
- A successful login creates a 32-byte random token. It is sent as the cookie `rb_session` (`HttpOnly; Secure` when the public URL is https; `SameSite=Strict; Path=/`), and the server stores its SHA-256.
- Sessions last 30 days, sliding: `last_seen_at` and `expires_at` are bumped at most once an hour.
- Logout deletes the session. Changing the password deletes every other session.
- **CSRF:** non-GET UI requests must carry an `Origin` header (or `Referer` if `Origin` is absent) that matches the request host. SameSite=Strict is the second layer.
- **Login throttle:** 5 failures per minute per client IP, then `429`. Counted in memory.
- **Client IP:** with `RUSTYBEE_TRUST_PROXY=1`, taken from `CF-Connecting-IP` when present, else from the **rightmost** `X-Forwarded-For` entry (the one the proxy appended; entries to its left are client-supplied). Otherwise, and when neither header parses, the peer address.

### Turnstile (optional)
- Enabled when both `RUSTYBEE_TURNSTILE_SITE_KEY` and `RUSTYBEE_TURNSTILE_SECRET` are set. Setting only one fails startup.
- It applies only to requests that came through Cloudflare: `RUSTYBEE_TRUST_PROXY=1` and a `CF-Connecting-IP` header is present. Cloudflare always sets that header and a client cannot remove it. Requests without it (LAN, or a non-Cloudflare proxy) skip Turnstile, so local login keeps working when the internet or tunnel is down.
- When it applies, the login page loads the Turnstile widget inside the password form and the code-send forms. The widget is meant to be created in **Invisible** mode in the Cloudflare dashboard. Those forms' buttons start disabled and are enabled once a token arrives.
- `POST /login` and `POST /login/code` then require the `cf-turnstile-response` field. The server checks it with Cloudflare Siteverify (`secret`, `response`, `remoteip` = client IP) before doing anything else except the login throttle. A missing or rejected token, a Siteverify error, timeout (10 s) or `success: false` all return `403` (fail closed). A Turnstile failure does not count toward the login throttle.
- `POST /login/verify` is not gated; it is already bounded by 5 tries per code and the login throttle.

### API keys
- Format: `rbk_` followed by 43 base64url characters. The secret is shown **once** at creation.
- The server stores its SHA-256 and a `prefix` (the first 12 characters) for display.
- Sent as `Authorization: Bearer <key>`. A key in the query string is rejected with `400 key_in_query`.
- Revoked keys get `401`. `last_used_at` is updated at most once a minute.

### Device tokens and pairing codes
- Defined in spec 002. Both are stored as SHA-256 only.
- Hashes are compared by looking up the indexed hash column. The tokens are 256-bit random, so this leaks no timing information.

## Acceptance criteria
1. AC-005-01 Login with the right password sets the cookie. A wrong password returns 401 and nothing is set.
2. AC-005-02 UI routes redirect to `/login` without a valid session. API routes return 401 JSON.
3. AC-005-03 A POST to a UI route with a foreign `Origin` returns 403.
4. AC-005-04 The 6th failed login within a minute returns 429.
5. AC-005-05 An API key works until it is revoked, then gets 401. `?api_key=` gets 400.
6. AC-005-06 After a password change, other sessions are invalid and the current one still works.
7. AC-005-07 With an admin phone set and a device connected, the password is refused (403), and a texted code logs in once.
8. AC-005-08 After `RUSTYBEE_OTP_MAX_FAILURES` code messages fail or time out unsent, the password is accepted, and the login resets the counter.
9. AC-005-09 With no device connected, or no admin phone, the password is accepted and a code request returns 409.
10. AC-005-10 A code dies after 5 wrong tries. A second send within 30 s returns 429.
11. AC-005-11 Code messages are missing from `GET /api/v1/messages`, return 404 from `GET /api/v1/messages/{id}`, and fire no webhooks.
12. AC-005-12 The admin phone is stored only after the code texted to it is entered in Settings.
13. AC-005-13 With Turnstile on and a `CF-Connecting-IP` header, `POST /login` and `POST /login/code` without a valid token return 403; with one Siteverify accepts, login succeeds and Siteverify gets the `CF-Connecting-IP` as `remoteip`. An unreachable Siteverify returns 403.
14. AC-005-14 With Turnstile on but no `CF-Connecting-IP` header, the login page has no widget and login works without a token.
15. AC-005-15 With `RUSTYBEE_TRUST_PROXY=1`, a spoofed leftmost `X-Forwarded-For` entry does not change the client IP used by the login throttle.

## Test plan
| AC | Test |
|---|---|
| 01–06 | `server/tests/auth.rs` |
| 07–12 | `server/tests/otp.rs` |
| 13–15 | `server/tests/auth.rs` |

## Assumptions
- Single admin with no username field.
- `rustybee set-password` run from the CLI deletes every session, since there is no current session to keep.
- The login throttle window (1 min) can be injected, for tests only.

## Open questions
