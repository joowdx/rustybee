# 004 — Message lifecycle

Status: implemented
Updated: 2026-10-17

## Design

### Outbound states
```
queued ──dispatch──▶ dispatched ──status sent──▶ sent ──status delivered──▶ delivered
  │                    │  │                        │
  │                    │  └─status delivered──────┼──────────────────────▶ delivered
  │                    └──status failed──▶ failed ◀──status failed────────┘
  ├──ttl──▶ expired    └──ttl (never acked)──▶ expired
  └──cancel──▶ failed (error_code=cancelled)
dispatched (acked) ──no status within 24 h──▶ failed (error_code=status_timeout)
expired ──status sent──▶ sent        expired ──status delivered──▶ delivered
```
- `domain::lifecycle::transition(from, event) -> Option<State>` is a pure function and the only place transitions are decided. Any pair not in the diagram returns `None`, which means "ignore and log at debug". A late or out-of-order status therefore never moves a message backwards.
- Terminal states are `delivered`, `failed` and `expired`. `sent` is terminal for pacing (spec 006) but can still become `delivered` or `failed`.
- `expired` is terminal for every server-side event, but a late `sent` or `delivered` status from the phone still moves it on: the `send` reached the phone and only its ack was lost, so the phone sent the message anyway. These go through the normal transition path, so the timestamp is set and `message.sent` or `message.delivered` fires once. `expired_at` is kept. A late `failed` is ignored.
- A transition sets the matching timestamp column and, for failures, `error_code` and `error_message`.
- **Each applied transition** to `sent`, `delivered`, `failed` or `expired` inserts one row into `webhook_deliveries` for every active webhook subscribed to that event, in the **same transaction** (spec 008). It also publishes a `MessageChanged` event on the in-process bus (SSE).

### Inbound
- Stored as `direction='in'`, `state='received'` and `received_at`, with `peer` set to the sender.
- The pair `(device_id, inbound_uid)` is unique. A duplicate insert is a no-op.
- Each new inbound message fires the `message.received` webhook.

### TTL sweeper
Runs every 60 s with `RUSTYBEE_MESSAGE_TTL_HOURS`, default 24:
- `queued` messages, and `dispatched` messages with no `device_acked_at`, older than the TTL become `expired`.
- `dispatched` messages with `device_acked_at` older than 24 h become `failed/status_timeout`.

### Batches
- A send request with N recipients creates one batch and N messages.
- Batch status is derived:
  - `pending` if any message is queued or dispatched;
  - otherwise `completed` if all are sent or delivered;
  - `failed` if none are;
  - `partial` otherwise.

### Segment math (`domain::sms`)
- Classify the body as GSM-7 (the basic table plus the extension table, where extension characters count as 2 septets) or UCS-2.
- Count UCS-2 by UTF-16 code units.
- Limits per encoding:

  | Encoding | Single message | Per part when split |
  |---|---|---|
  | GSM-7 | 160 septets | 153 |
  | UCS-2 | 70 units | 67 |

- `segments(body) -> {encoding, units, parts, remaining}`. The same algorithm is used in `static/app.js` for the live counter.

### Phone number normalisation
- Trim the input and remove spaces, `-`, `(`, `)` and `.`.
- A local Philippine mobile number, exactly `09` followed by 9 digits (`09XXXXXXXXX`), becomes `+639XXXXXXXXX`: the leading `0` is replaced with `+63`. Nothing else is rewritten.
- It must then match `^\+?[0-9]{3,20}$`, or it is rejected as `invalid_recipient`.
- Alphanumeric peers are allowed for **inbound** only.

## Acceptance criteria
1. AC-004-01 Every transition in the diagram is applied. Every other pair is refused, and an exhaustive test covers all of them.
2. AC-004-02 Applying the same status twice produces exactly one webhook row and one timestamp.
3. AC-004-03 A `delivered` status arriving before `sent` ends in `delivered`. A later `sent` is ignored.
4. AC-004-04 The sweeper expires old `queued` and unacked `dispatched` messages, and fails acked ones after 24 h.
5. AC-004-05 Segment counts are correct for a GSM-7 body of 160 characters (1 part), 161 characters (2 parts), a body containing `€` (each `€` counts as 2), an emoji (UCS-2) and a body of 70 Cyrillic characters (1 part) or 71 (2 parts).
6. AC-004-06 Phone normalisation accepts `+1 (555) 123-4567` and rejects `abc`.
7. AC-004-07 `0917 123 4567` normalises to `+639171234567`; `0917123456` (too short) and `+639171234567` are left as they are.
8. AC-004-08 An `expired` message that gets a `sent` or `delivered` status moves to that state, sets its timestamp and fires its webhook once. No other event moves it.

## Test plan
| AC | Test |
|---|---|
| 01, 03 | `domain::lifecycle` unit tests |
| 02 | store integration test |
| 04 | sweeper test with an injected clock |
| 05 | `domain::sms` unit tests |
| 06 | `domain::phone` unit tests |
| 07 | `domain::phone` unit tests |
| 08 | `domain::lifecycle::ac_004_08_*`; `server/tests/device_ws.rs::ac_004_08_expired_message_takes_late_sent` |

## Assumptions
- No user-facing cancel in the first release. The `cancel` transition exists only for removing a device that still has `queued` or `dispatched` messages; history is kept (spec 003 soft delete).

## Open questions
