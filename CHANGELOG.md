# Changelog

Downloads and install notes are on the
[Releases](https://github.com/ReaMesh/ReaMesh/releases) page.

## 1.1.0

<sub>October 21, 2026</sub>

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="proprietary/changelog/1.1.0-banner-dark.png">
  <img alt="ReaMesh 1.1: a REAPER arrange view with Lena's, Sam's and Alex's cursors, a dashed outline where Sam's item will land, and items outlined in the color of the person editing them" src="proprietary/changelog/1.1.0-banner-light.png">
</picture>

Everyone in a session needs ReaMesh 1.1. It can't join sessions hosted with 1.0,
and 1.0 can't join sessions hosted with 1.1. The Lobby greys out sessions from
another version and says who has to update.

### See where the others are working

You now see the other people's mouse pointers inside REAPER's arrange view, with
their name and color. A pointer fades out when its owner switches to an FX
window, the mixer or the MIDI editor, and comes back when they return to the
arrange.

<img alt="Lena's cursor with her name tag over a MIDI item, and an audio item dimmed and outlined because someone else is editing it" src="proprietary/changelog/1.1.0-cursors.png">

Items someone else is editing are dimmed and outlined in that person's color.
When someone drags items, you see a dashed outline of where they will land.
Volume, pan, width and the plug-in parameter someone is turning update live while
they hold the mouse button, not only after they let go.

If you follow someone, their pointer is highlighted and your view keeps up with
the items they drag. If you edit an item that someone else is already working on,
your change snaps back right away.

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="proprietary/changelog/1.1.0-editing-outline-dark.png">
  <img alt="Ben's and Lena's REAPER windows side by side: Lena's cursor and a pink outline on the item she is editing appear on Ben's screen, with a notice that Lena is editing Lead Vox" src="proprietary/changelog/1.1.0-editing-outline-light.png">
</picture>

Two new settings let you hide the others' pointers or stop sharing your own.

### Find sessions over Bluetooth

Some networks, guest Wi-Fi for example, block the discovery ReaMesh uses to list
sessions in the Lobby. The app can now use Bluetooth to find hosts nearby, on
macOS and Windows. It stays off until you turn it on: the Lobby asks once, and the
switch is in Settings. Bluetooth only carries the host's network address. The
session itself still runs over the network, so both computers need to reach each
other there.

### Other changes

- Marker and region ruler lanes are synced on REAPER 7.19 and later.
- Edits in large projects go out faster, and big updates are compressed.
- Settings has a new Joined projects list with the copies of projects you joined,
  their size and when you last used them. You can open a copy's folder or move it
  to the Trash (Recycle Bin on Windows). Unused copies are now kept for 7 days
  instead of 3.
- On macOS, Settings opens in its own window and has the same options as on
  Windows.
- Guests who can only reach each other through the host, for example after joining
  by invite link, now see each other in the session.
- Hosting again no longer leaves an old second entry in other people's Lobby.
- On first setup the extension is installed when you click Install, so the setup
  screen can tell you to restart REAPER.

### Security

- When a session has a passphrase, the host now has to prove it knows the
  passphrase before you download anything. A copied session with the same name can
  no longer pick up a working key from people who type the passphrase into it.
- The connection between the app and REAPER needs a key that changes every time
  REAPER starts, so other programs on your computer can't control ReaMesh.
- Invite links ignore local-only addresses and wait until you click Join.
- Incoming changes are checked before they reach REAPER, and media paths that
  point outside the project folder are refused.

### Good to know

On macOS, the system may ask for Bluetooth permission again after this update.

**Full changelog**: [v1.0.0...v1.1.0](https://github.com/ReaMesh/ReaMesh/compare/v1.0.0...v1.1.0)

## 1.0.0

<sub>October 15, 2026</sub>

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="proprietary/changelog/1.0.0-banner-dark.png">
  <img alt="ReaMesh 1.0: a REAPER project during playback with the floating people list showing who is on which track, and an item in grey because someone else is editing it" src="proprietary/changelog/1.0.0-banner-light.png">
</picture>

The first release. ReaMesh lets several people work on one REAPER project at the
same time, each in their own REAPER, on macOS and Windows. There is no account and
no server: computers connect directly over a local network or VPN.

### Host and join

One person hosts their open project, and the others get a local copy and work on
it live.

- Sessions on your network show up in the Lobby by themselves. For a VPN, or a
  network where discovery doesn't work, send an invite link: clicking it opens
  ReaMesh on the join screen. It works as long as both computers can reach each
  other directly.
- Before you join, you see who is hosting, how big the project is and which of its
  plug-ins you are missing. If you don't have them, you can join without effects in
  view-only mode.
- Only the files the project uses are shared: the project file, its media, files
  that plug-in state points to, and sub-projects. Everything else in the folder
  stays private.
- Downloads come from everyone in the session, and files you already have are
  skipped. Large projects open right away with placeholder items while the media
  keeps coming in. You can look around, and editing unlocks when loading is done.

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="proprietary/changelog/1.0.0-joining-dark.png">
  <img alt="The ReaMesh app: sessions on the network, the check before joining with host, size and plug-ins, the project download, and the session with its people and invite code" src="proprietary/changelog/1.0.0-joining-light.png">
</picture>

### What stays in sync

Edits reach the others in about a second.

| Part of the project | Shared edits |
| --- | --- |
| Tracks | Add, remove, reorder, names, volume, pan, colors, folders and lane settings |
| Audio and MIDI items | Move, split, trim, fades, takes, take FX and MIDI content |
| Effects and automation | Track FX, input FX, plug-in state and track envelopes |
| Routing and groups | Sends, receives, send envelopes and group/VCA assignments |
| Project settings | Tempo, time signatures, markers, regions, notes, title, author and timebase |
| Media files | New recordings and imported files transfer in the background |

Track mute, solo, record arm, monitoring and window layout stay yours. The master
track, hardware outputs, pooled automation items, marker ruler lanes and project
extension state aren't synced.

### Work on the same project without conflicts

Whoever edits something first holds it until two seconds after their last change.
If someone else changes it in that time, their edit snaps back and they see who is
working on it. This applies to the host as well. Items others are editing turn grey
on your screen.

Other people's edits don't add undo points, so Ctrl+Z (Cmd+Z on macOS) starts with
your own work. REAPER's undo restores whole snapshots though, so undoing an earlier
action can roll back changes you received, and that rollback syncs too.

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="proprietary/changelog/1.0.0-editing-lock-dark.png">
  <img alt="Ben's and Lena's REAPER windows side by side: the item Lena is editing is grey on Ben's screen, with a notice that Lena is editing Lead Vox" src="proprietary/changelog/1.0.0-editing-lock-light.png">
</picture>

### See who is there

- A small floating list over REAPER shows who is connected and which track they
  are on.
- Follow someone to keep up with their playback, selected track, view, item
  selection and open effect window.
- Ping a spot on the timeline. Everyone can jump to it or turn it into a marker.
- Hosts can remove a guest, or block their device for 30 minutes.
- If the host's computer drops out, guests reconnect on their own when the session
  comes back.

### Private by choice

- Add a passphrase to a session and only people who know it can download, edit or
  appear in the session. Session messages are then encrypted too. Session names stay
  visible on the network, and open sessions don't have this extra encryption.
- Only join people you trust: a shared project can load plug-ins on your computer.
  ReaMesh asks before you join, and checks the downloaded project for plug-ins the
  host didn't list before opening it.

### The ReaMesh app

- The app installs the REAPER extension for you. You need REAPER 7.12 or later.
- macOS 11 or later (Apple silicon and Intel) and 64-bit Windows.
- English and German.
- Updates download in the background and install when you restart the app, from the
  Stable or Beta channel.

<sub>Backgrounds by Pawel Czerwinski and Andrew Kliatskyi on Unsplash.</sub>
