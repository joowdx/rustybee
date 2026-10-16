# 012 — Android SMS send and receive

Status: implemented
Updated: 2026-10-16

## Design

### Send (`Sender`)
1. When a `send` frame arrives, apply the dedupe rules from spec 002 using the `sent` table, then ack it.
2. Pick the `SmsManager` with `context.getSystemService(SmsManager::class.java)`:
   - `.createForSubscriptionId(sim)` when `sim` is not null and is an active subscription;
   - otherwise use `config.sim` from the server if set;
   - otherwise the system default.
   - If the requested `sim` is not active, report `failed`/`sim_unavailable`.
3. Split the body with `divideMessage(body)` into `parts` (n). Store `parts=n`.
4. For each part `i`, create a sent PendingIntent and, when delivery reports are wanted (always), a delivered one:
   - explicit `Intent(context, SmsStatusReceiver::class.java)`;
   - action `dev.joowdx.rustybee.SMS_SENT` or `…SMS_DELIVERED`;
   - extras `msg_id` and `part`;
   - request code taken from a monotonically increasing int stored in prefs, so every part gets a unique code;
   - flags `FLAG_ONE_SHOT` plus `FLAG_IMMUTABLE` for sent intents and `FLAG_MUTABLE` for delivered intents: the radio fills in the `pdu`/`format` extras, which an immutable PendingIntent drops. The intent is explicit, so only extras can be filled in.
5. Call `sendMultipartTextMessage` (or `sendTextMessage` when n = 1). A `SecurityException` or `IllegalArgumentException` reports `failed` immediately.
6. **Combining part results** (`SmsStatusReceiver` → `goAsync()` → `Sender.onPartResult`, persisted in `sent`):
   - **Sent result:** if the resultCode is `RESULT_OK`, `parts_sent++`; otherwise `parts_failed++` and the error is mapped (below).
   - When `parts_sent + parts_failed == parts`:
     - any failure → state `failed` and a `status failed` frame;
     - otherwise → state `sent` and a `status sent` frame.
   - **Delivered result:** read the PDU status with `SmsMessage.createFromPdu(pdu, format).status`.
     - status ≥ 64 (`0x40`) is a permanent failure: `failed`/`delivery_failed`, sent at once, unless the message is already failed.
     - status < 32 is a success: `parts_delivered++`. Once `parts_delivered == parts` and the state is `sent`, report `delivered`.
     - 32–63 means pending: ignore it.
   - Every status frame goes through `Uplink` (the outbox).
7. **Error mapping:**

   | Result | Code |
   |---|---|
   | `RESULT_ERROR_GENERIC_FAILURE` | `generic_failure` |
   | `RESULT_ERROR_NO_SERVICE` | `no_service` |
   | `RESULT_ERROR_NULL_PDU` | `null_pdu` |
   | `RESULT_ERROR_RADIO_OFF` | `radio_off` |
   | `RESULT_ERROR_LIMIT_EXCEEDED` | `limit_exceeded` |
   | `RESULT_ERROR_SHORT_CODE_NOT_ALLOWED` | `short_code_not_allowed` |
   | `RESULT_ERROR_SHORT_CODE_NEVER_ALLOWED` | `short_code_never_allowed` |
   | any other | `unknown_<code>` |

   The `message` field carries a short English text.
8. If the process dies between sending and the callbacks arriving, the `sent` row stays `accepted`. On the next service start, any row that has been `accepted` for more than 10 min is reported `failed`/`generic_failure` with the message "outcome lost". It is never sent again.

### Receive (`SmsInboundReceiver`)
- **Manifest:** `<receiver android:exported="true" android:permission="android.permission.BROADCAST_SMS">` with an intent filter for `android.provider.Telephony.SMS_RECEIVED`.
- **`onReceive`:** `goAsync()`, then `Telephony.Sms.Intents.getMessagesFromIntent`.
  - Group the PDUs by originating address and concatenate the bodies in order.
  - `received_at` is the first PDU's `timestampMillis`.
  - `sim` comes from the intent's `SubscriptionManager.EXTRA_SUBSCRIPTION_INDEX` extra, or `-1` mapped to null.
- **Skipped** if the gateway is disabled, `config.receive_enabled=false`, the phone's local forwarding "off" is set, or the filters drop the message (spec 002 semantics).
- **Otherwise:**
  1. Compute the uid from spec 002.
  2. Insert an `inbound` frame into the outbox. Inserting a uid that is already queued in the outbox is a no-op.
  3. Start or nudge `GatewayService` with `startForegroundService`. Receiving `SMS_RECEIVED` grants a temporary FGS start exemption.
  4. Call `finish()`.

### Status receiver
`SmsStatusReceiver` is declared `exported="false"`. It is only ever targeted by explicit PendingIntents.

## Acceptance criteria
1. AC-012-01 Combining parts: 3 parts with results OK, OK, failure gives one `failed` status. OK, OK, OK gives `sent`. Three delivered successes then give `delivered`.
2. AC-012-02 A `send` frame received again after a terminal outcome sends the SMS 0 more times and re-reports the stored status.
3. AC-012-03 The uid matches the spec 002 fixture `inbound.json`.
4. AC-012-04 The filter semantics match spec 002 for all three modes and all four operations.
5. AC-012-05 On the emulator, an SMS injected with `adb emu sms send` reaches the server as `inbound`. Manual check.
6. AC-012-06 On the emulator, an API send reaches `sent`. The emulator also reports `delivered`. Manual check.
7. AC-012-07 Another app cannot send `SMS_RECEIVED` to the receiver, because of the `BROADCAST_SMS` guard. Checked by reviewing the manifest.

## Test plan
| AC | Test |
|---|---|
| 01 | `PartAggregatorTest` (pure Kotlin) |
| 02 | `SendDedupeTest` |
| 03 | `InboundUidTest` |
| 04 | `FilterTest` |
| 05, 06 | Manual check in M7 |
| 07 | Manifest assertion test that parses the merged manifest |

## Assumptions
- Delivery reports are always requested. They cost nothing in battery, and carriers that don't support them simply never report.
- A delivered callback without a PDU counts as success when its resultCode is `RESULT_OK`, otherwise as pending.
- A duplicate `send` for a message already `delivered` re-reports `sent` and then `delivered`, so the server never sees `dispatched → delivered`. `sent` and `failed` re-report just that state.
- If `config.sim` is set but not active, the system default SMS subscription is used. Without `READ_PHONE_STATE` no subscription counts as active, so an explicit `sim` fails with `sim_unavailable`.
- `hello.sims[].number` is always null: reading it needs `READ_PHONE_NUMBERS`, which is not requested.

## Open questions
