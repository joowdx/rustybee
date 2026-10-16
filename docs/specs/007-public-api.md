# 007 — Public API

Status: implemented
Updated: 2026-10-17

## Design
- **Base:** `/api/v1`, JSON in and out, authenticated with `Authorization: Bearer rbk_…` (spec 005). The request body limit is 2 MB.
- **Errors:** `{"error":{"code":"<snake>","message":"…"}}`, with status codes:

  | Status | Codes |
  |---|---|
  | 400 | `bad_request`, `invalid_recipient`, `body_too_long`, `device_required`, `key_in_query` |
  | 401 | `unauthorized` |
  | 404 | `not_found` |
  | 409 | `device_disabled` |
  | 413 | (payload too large) |
  | 429 | `queue_full` |

### Message object
```json
{"id":"01J…","batch_id":"01J…"|null,"device_id":"01J…","direction":"out"|"in","peer":"+15551234567",
 "body":"…","sim":null,"state":"queued|dispatched|sent|delivered|failed|expired|received","parts":1|null,
 "error":{"code":"…","message":"…"}|null,
 "created_at":"2026-10-16T08:00:00.000Z","dispatched_at":null,"sent_at":null,"delivered_at":null,
 "failed_at":null,"expired_at":null,"received_at":null}
```
Timestamps are RFC 3339 UTC strings with milliseconds.

### Endpoints
| Method | Path | Body / query | Response |
|---|---|---|---|
| GET | `/api/v1/health` | — (no auth) | `200 {"ok":true,"version":"x.y.z"}` |
| POST | `/api/v1/messages` | `{to:[str] 1..=1000, body:str 1..=1600 chars, device_id?, sim?}` | `202 {batch_id, messages:[Message]}` |
| POST | `/api/v1/batches` | `{items:[{to, body}] 1..=1000, device_id?, sim?}` | `202 {batch_id, messages:[Message]}` |
| GET | `/api/v1/batches/{id}` | — | `{id, device_id, total, counts:{queued,dispatched,sent,delivered,failed,expired}, status}` |
| GET | `/api/v1/messages` | `direction, state, device_id, peer, batch_id, q (substring of body or peer), since, until (RFC 3339), limit (1..=200, default 50), cursor` | `{items:[Message], next_cursor:str\|null}` |
| GET | `/api/v1/messages/{id}` | — | `Message` |
| GET | `/api/v1/devices` | — | `[{id, name, enabled, is_default, connected, last_seen_at, model, app_version, battery, charging, network, sims}]` |

- Duplicate recipients within one request are kept. The caller decides.
- **Paging:** keyset over `id DESC`. The cursor is the opaque base64url of the last row's id.
- All validation happens before anything is written. An invalid recipient rejects the whole request and names its index: `message: "to[3]: …"`.
- Recipients need at least 7 digits (spec 004). `RUSTYBEE_ALLOW_SHORT_CODES=1` lowers that to 3 so short codes can be texted.
- **Backlog cap:** a send is refused with 429 `queue_full`, writing nothing, when the chosen device's `queued` messages plus the new ones would exceed `RUSTYBEE_MAX_QUEUED` (default 1000). The web send forms use the same path.

## Acceptance criteria
1. AC-007-01 `POST /messages` with 3 recipients returns 202, one batch and 3 queued messages, and wakes the dispatcher.
2. AC-007-02 An invalid recipient returns 400 `invalid_recipient` with its index, and nothing is written.
3. AC-007-03 Listing with `limit=2` pages through every row exactly once using `next_cursor`.
4. AC-007-04 The filters `direction`, `state`, `peer`, `q`, `since` and `until` each narrow the results correctly.
5. AC-007-05 A request without a key returns 401. `/health` needs no key.
6. AC-007-06 Batch counts and status follow spec 004.
7. AC-007-07 With `RUSTYBEE_MAX_QUEUED=3` and 2 messages queued, sending 2 more returns 429 `queue_full` and writes nothing; sending 1 more succeeds.
8. AC-007-08 A recipient with fewer than 7 digits, such as `2336`, returns 400 `invalid_recipient`, unless `RUSTYBEE_ALLOW_SHORT_CODES=1`.

## Test plan
| AC | Test |
|---|---|
| 01–08 | `server/tests/api.rs` |
| 08 | `domain::phone` unit tests |

## Assumptions
- No scheduled sends in the first release (non-goal).
- `body` is limited to 1600 characters, about 10 GSM segments.

## Open questions
