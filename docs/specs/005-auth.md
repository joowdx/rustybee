# 005 — Authentication

Status: approved (self, one-shot)
Updated: 2026-10-16

## Design

### Admin password
- `rustybee set-password` reads the password twice from the TTY, or once from stdin with `--stdin`. It needs at least 10 characters and stores an Argon2id hash with default params in `settings.admin_password`.
- On `serve`, if no password is set and `RUSTYBEE_ADMIN_PASSWORD` is present, the server hashes and stores it once. If neither exists, the UI shows "run `rustybee set-password`" and login is disabled.

### Sessions
- A successful login creates a 32-byte random token. It is sent as the cookie `rb_session` (`HttpOnly; Secure` when the public URL is https; `SameSite=Strict; Path=/`), and the server stores its SHA-256.
- Sessions last 30 days, sliding: `last_seen_at` and `expires_at` are bumped at most once an hour.
- Logout deletes the session. Changing the password deletes every other session.
- **CSRF:** non-GET UI requests must carry an `Origin` header (or `Referer` if `Origin` is absent) that matches the request host. SameSite=Strict is the second layer.
- **Login throttle:** 5 failures per minute per client IP, then `429`. Counted in memory.
- **Client IP:** taken from `X-Forwarded-For` only when `RUSTYBEE_TRUST_PROXY=1`; otherwise the peer address.

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

## Test plan
| AC | Test |
|---|---|
| 01–06 | `server/tests/auth.rs` |

## Assumptions
- Single admin with no username field.

## Open questions
