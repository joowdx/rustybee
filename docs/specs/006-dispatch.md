# 006 — Dispatch, pacing and replay

Status: implemented
Updated: 2026-10-17

## Design

### Hub
`Hub` holds `HashMap<device_id, DeviceHandle>`. A `DeviceHandle` carries:
- an outbound frame sender (mpsc);
- a `Notify` for the dispatcher;
- a connection generation number.

### Dispatcher
There is one tokio task per connected device. It is started after `welcome` and stopped when the connection closes. Its loop:
1. **Replay first.** Re-send every `dispatched` message without `device_acked_at`, oldest first, with its stored `frame_id`.
2. **Window of 1.**
   - At most one outbound message per device is *in flight*: dispatched but neither terminal-for-pacing (`sent`, `delivered` or `failed`) nor timed out.
   - The in-flight timeout is 120 s after `device_acked_at`, or 120 s after sending if no ack arrives.
   - When it times out, the dispatcher moves on. The message stays `dispatched`, and the sweeper rules from spec 004 apply.
   - On a new connection the window is seeded from the newest `dispatched` row using its stored `device_acked_at` (or `dispatched_at`), so only the remaining part of the timeout is waited. A row whose timeout already ran out is not seeded.
3. **Pacing.**
   - The next `send` waits at least `send_delay_ms` after the previous one left the in-flight state, measured from the server's monotonic clock.
   - The default delay is 5000 ms, clamped to 0–3,600,000.
4. **Pick the next message:** the oldest `queued` row for the device (`ORDER BY id`).
   - In one write transaction: set `state=dispatched`, `dispatched_at`, and a new `frame_id` ULID.
   - Then send the `send` frame.
5. **Sleep** until `Notify` fires (new message queued, status received, ack received, config changed) or the next pacing or timeout deadline comes up.
6. **Errors.** A store error does not end the task. The loop starts again from step 1 after a pause of 250 ms, doubling up to 30 s (reset after a run of more than 30 s). It ends only when the socket is gone.

### Choosing the device for a new message
- Use the explicit `device_id` if the request gives one; it must exist and be enabled.
- Otherwise use the device with `is_default=1`.
- Otherwise use the only enabled device.
- Otherwise reject with `400 device_required`.
- Messages queue even when the device is offline. They go out on the next connect, or the FCM wake (spec 009) brings the device back.

### Acks and statuses from the device
- An `ack` whose `ref` matches a message's `frame_id` sets `device_acked_at`. Acks for other frames (`welcome`, `config`) are ignored.
- A `status` frame applies the spec 004 transition, acks the frame, and fires `Notify`.
- A `nack` whose `ref` matches a message's `frame_id` fails the message with error `nack_<code>` and fires `Notify`; the frame is never resent.
- A disabled device still connects and reports statuses, but its dispatcher sends nothing until it is enabled again; its queued messages wait.

### Disconnect
- Set `connected=0` and `last_seen_at`.
- Stop the dispatcher and remove the handle, but only if its generation still matches. A newer socket may already have replaced it.

### Restart safety
All dispatch state lives in SQLite. After a server restart, the replay in step 1 covers every unacked frame.

## Acceptance criteria
1. AC-006-01 Three queued messages with `send_delay_ms=200` reach a fake device in order, at least 200 ms apart, and never more than one in flight.
2. AC-006-02 A device that never sends a status still receives the next message after the 120 s in-flight timeout. Tests use a configurable timeout.
3. AC-006-03 Disconnecting after `send` and before `ack`, then reconnecting, re-sends the same `frame_id` and `msg_id`.
4. AC-006-04 A message queued while the device is offline is sent within 1 s of the device's `hello`.
5. AC-006-05 Device choice follows: explicit, then default, then the only enabled device, then 400.
6. AC-006-06 After a server restart, unacked dispatched messages are replayed.
7. AC-006-07 After a store error inside the dispatcher, the same socket still receives newly queued messages.

## Test plan
| AC | Test |
|---|---|
| 01–07 | `server/tests/dispatch.rs` with a fake device client and a short timeout config |
| 01, 02, 04 | `domain::pacing` unit tests |

## Assumptions
- A window of 1 per device is enough throughput for personal use. Carriers throttle anyway, and AOSP prompts after 30 SMS in 30 minutes.

## Open questions
- AC-006-07, the retry loop and the seeding of the window from stored times (AC-006-04) came from the 2026-10-17 review.
