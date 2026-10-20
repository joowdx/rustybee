# 015 — Network details

Status: implemented
Updated: 2026-10-20

## Goal
Let the phone report, per radio, the network name and signal strength, so the web UI shows which Wi-Fi or carrier the gateway is on and how strong the signal is. Every part is opt-in on the phone.

## Design

### Phone settings
- A "Network" screen, opened from the main screen next to "Forwarding", with two sections:
  - **Cellular**: "Carrier name" and "Signal strength" (noted as bars, and 2G to 5G while on mobile data).
  - **Wi-Fi**: "Network name" and "Signal strength".
- All four switches are off by default. They are phone-only preferences; the server cannot change them.
- The network type (`network`, spec 002) is not a setting and is always reported.
- Turning on the Wi-Fi network name first shows a dialog explaining that location access is used only to read the Wi-Fi name, also in the background, and never the location; "Not now" skips the prompts. "Continue" asks for precise location, then for "Allow all the time", because Android only reveals the Wi-Fi name to a background service that holds both, with Location turned on. If either is missing the switch stays on, the name is left out, and the section says what is missing with a button to grant it.

### Protocol
`hello` and `health` gain an optional `radios` object:

```json
"radios": {
  "cellular": {"name": "Example Mobile", "level": 3, "dbm": -95, "tech": "5G"},
  "wifi": {"name": "HomeNet", "level": 4, "dbm": -55}
}
```

- `cellular` describes the data SIM; it is present while a SIM is in service and one of its switches is on. `wifi` is present while Wi-Fi is connected and one of its switches is on.
- `name` is sent only with the name switch on; `level` (0–4 bars) and `dbm` only with the signal switch on. Any field may be missing when the phone can't read it.
- `cellular.tech` is the generation the status bar shows, `2G`, `3G`, `4G` or `5G` (5G includes 5G on an LTE anchor), sent with the Cellular signal switch and only while the default network is cellular (`network` is `cellular`), like the status bar; on Wi-Fi it is left out even with mobile data switched on. Reading 5G on an LTE anchor needs `READ_PHONE_STATE`; without it the plain data network type is used when readable.
- An app that supports this sends `radios` in every `hello` and `health`, as `{}` when nothing is on. The server stores it as sent, replacing what it had, so turning a switch off clears it on the server. A `health` without `radios` (an older app) leaves the stored value unchanged; a `hello` without it clears it. A `radios` that doesn't parse counts as absent and never fails the frame.

### When the phone reports
- At once (as a `health`, once welcomed) when a switch changes, a reported name changes, or the default network moves between Wi-Fi and cellular (`tech` appears or goes). Readings that change before `welcome` are checked again once welcomed.
- When a switch starts a listener (or the data SIM moves it), the report waits up to 2 seconds for the listener's first readings, so a half-read state (Wi-Fi missing, or 4G before the 5G display info) is never sent.
- On a change of bar `level` or `tech`: at most once a minute. A change inside the minute is sent when the minute is up, with the latest values. `dbm` alone never triggers a report.
- Otherwise with every `hello` and `health` (spec 002).
- Listeners run only while a switch that needs them is on: the cellular one (service state, so a carrier change is seen, plus signal while the Cellular signal switch is on) while either Cellular switch is on, on the data SIM and moved when the data SIM changes; the Wi-Fi one while either Wi-Fi switch is on, asking Android for location info (the name) only while the name switch is on, following one Wi-Fi network at a time.

### Web UI and API
- On `/` and `/devices`, each device card's status bar shows `{Wi-Fi name} (Wi-Fi signal) {carrier name} {tech} (cellular signal)`: Wi-Fi then cellular, each as its name, then for cellular its `tech`, then its icon (Wi-Fi arcs or cellular bars) with `level` lit when the signal is reported, and `dbm` in its title. A radio shows when it is reported or in use (on Wi-Fi the SIM stays registered for SMS); a radio without a reported name shows its icon only. `tech` shows only while `network` is `cellular`, so an older app that still sends it on Wi-Fi never shows a generation there. A connection that is not a radio (`ethernet`, `other`) shows first, as its type and a plain network icon.
- `GET /api/v1/devices` adds `radios`, as stored, or `null`.
- The server cleans `radios` before storing: `name` trimmed and cut to 64 characters (empty dropped), `level` clamped to 0–4, a `dbm` outside -150..-1 dropped, and a `tech` other than 2G–5G, or on `wifi`, dropped.

## Acceptance criteria
1. AC-015-01 `protocol/fixtures/hello.json` carries `radios` with both radios and `health.json` with `cellular` only; both suites round-trip them (AC-002-01).
2. AC-015-02 Server: a `hello` with `radios` stores it; a `health` with `radios` replaces it, `{}` included; a `health` without `radios` keeps it; a `hello` without it clears it.
3. AC-015-03 Server: on `/` and `/devices`, a device on Wi-Fi with `radios` shows the Wi-Fi name then its signal with `level` lit, then the carrier name and its bars, without `tech`; on cellular the carrier name, its `tech` and its bars; without `radios` only the plain icon of the connection in use shows, with no label or level.
4. AC-015-04 Server: `GET /api/v1/devices` returns `radios`, cleaned as described above; a malformed `radios` is ignored and the frame still acked.
5. AC-015-05 App: all four switches default to off, and with all off `radios` is `{}`.
6. AC-015-06 App: `radios` holds only the fields whose switch is on; a radio with nothing to report is left out.
7. AC-015-07 App: a switch or name change reports at once; a `level` change reports at most once a minute, with a trailing report; a `dbm`-only change reports nothing.
8. AC-015-08 App: a Wi-Fi name that is unknown (`<unknown ssid>`, permission or Location missing) is left out, and surrounding quotes are stripped.
9. AC-015-09 Manual, on a real phone: each switch shows and hides its field in the web UI within a minute; walking away from the router lowers the Wi-Fi bars.
10. AC-015-10 App: the generation maps 2G–5G network types (5G on an LTE anchor included) and is sent with the signal switch only and only while the default network is cellular; a generation change reports at most once a minute, and moving between Wi-Fi and cellular at once.
11. AC-015-11 App: while a newly started listener settles (up to 2 seconds), a change is held back until then instead of going at once.

## Test plan
| AC | Test |
|---|---|
| 01 | `server/tests/fixtures.rs`; app `ProtocolFixturesTest` |
| 02, 04 | `server/tests/device_ws.rs::ac_015_02_*`, `server/tests/device_ws.rs::ac_015_04_*`, `server/tests/api.rs::ac_015_04_*` |
| 03 | `server/tests/ui.rs::ac_015_03_*` |
| 05–08, 10, 11 | app `RadiosTest` |
| 09 | Manual |

## Open questions
- Built on "go ahead" without a spec review (2026-10-17); the choices above (phone-only switches, off by default, bar changes at most once a minute, background location for the Wi-Fi name) were agreed in chat. The cellular generation (`tech`, with the signal switch, only while mobile data is on) was added on "go ahead" the same day. On 2026-10-20, from a field report (5G shown while on Wi-Fi), "while mobile data is on" became "while on mobile data".
