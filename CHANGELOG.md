# Changelog

What changed in each release, newest first. The release workflow uses a version's section as its release notes, so every release needs one.

## 0.3.7 (2026-10-01)

The first public release. It's early: MeltAlarm has been tested on an MSI MPG Ai1300TS under Windows 11. The Ai1600TS uses the same protocol and Windows 10 should work. If you run either, please open an issue and say how it went.

**Works only with the MSI MPG Ai1300TS and Ai1600TS power supplies, connected by USB.** With any other PSU it shows a message and exits.

Download `meltalarm.exe` and run it. The [README](https://github.com/DeniDoman/meltalarm#readme) covers installing, what the alerts mean and what to do when the alarm goes off.

The exe isn't code-signed, so Windows SmartScreen will warn you. To check the file, compare `Get-FileHash .\meltalarm.exe` with `SHA256SUMS`.

For now the settings live in the tray icon's right-click menu. A Settings window is planned.
