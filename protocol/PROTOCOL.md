# rustybee device protocol

The normative spec is [`docs/specs/002-device-protocol.md`](../docs/specs/002-device-protocol.md).

`fixtures/` holds golden frames. Both the server (`server/tests/fixtures.rs`) and the app (`ProtocolFixturesTest`) parse every file and round-trip it.

| File | Direction | Type |
|---|---|---|
| `pair_request.json` / `pair_response.json` | HTTP | pairing |
| `hello.json`, `status_*.json`, `inbound.json`, `health.json`, `set_filters.json` | device → server | |
| `welcome.json`, `config.json`, `send.json` | server → device | |
| `ack.json`, `nack.json` | both | |

`inbound.json`'s `uid` is the spec 002 hash of its own fields, so it doubles as the uid test vector.
