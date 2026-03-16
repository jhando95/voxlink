# Screen Share V1

## Goal

Add an efficiency-safe screen share mode to Voxlink without regressing idle CPU, memory, or voice smoothness.

## Product Scope

- One active screen share per room or voice channel.
- Primary display capture only in v1.
- No system audio capture.
- Start and stop from the existing call view.
- Viewers see the live share inline in the room view.
- No background capture, encoding, decoding, or extra tasks when nobody is sharing.

## Efficiency Constraints

- Keep the existing voice path untouched.
- Start capture only after the server grants the share slot.
- Stop capture immediately on stop, leave, disconnect, or room teardown.
- Bound all outgoing and incoming screen frame queues to latest-only behavior.
- Cap stream quality to a lightweight desktop-friendly preset:
  - JPEG frames
  - Max width 960px
  - 8 fps
  - Quality 55

## Protocol

### Signal Messages

- `StartScreenShare`
- `StopScreenShare`
- `ScreenShareStarted { sharer_id, sharer_name, is_self }`
- `ScreenShareStopped { sharer_id }`

### Binary Media Packets

- Packet kind `1`: audio
- Packet kind `2`: screen frame

Client to server:

- Audio: `[kind, audio_payload...]`
- Screen: `[kind, jpeg_payload...]`

Server to client:

- Audio: `[kind, sender_id_len, sender_id_bytes..., audio_payload...]`
- Screen: `[kind, sender_id_len, sender_id_bytes..., jpeg_payload...]`

## Client Architecture

- `app_desktop::screen_share`
  - Owns the local capture session.
  - Uses `xcap` to capture the primary monitor on a detached thread.
  - Downscales and JPEG-encodes frames before enqueueing them.
  - Stops fully when the share ends.
- `net_control`
  - Adds a bounded incoming screen-frame queue.
  - Adds `send_screen_frame()` and `parse_screen_frame()`.
- `tick_loop`
  - Drains only the latest pending screen frame at a throttled cadence.
  - Decodes and updates the room image only when a share is active.

## Server Architecture

- Reuse the existing `Room` model.
- Add one room-level active sharer slot.
- Reject a second share request while one is already active.
- Relay screen frames only to other peers in the same room.
- Clear the active share when the sharer stops, leaves, changes channel, or disconnects.

## UI

- Add a `Screen Share` control tile/button in the room view.
- Add a stage card that shows:
  - idle placeholder when no one is sharing
  - sharer name when live
  - live image when frames arrive

## Non-Goals

- Window picker
- System audio share
- Multiple concurrent shares
- Webcam video
- Recording
