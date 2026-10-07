# Getting help

- Usage questions: [GitHub Discussions](https://github.com/neerajsunil/pip-recorder/discussions).
- Reproducible defects: [bug report](https://github.com/neerajsunil/pip-recorder/issues/new?template=bug_report.yml).
- Ideas and planned platform requests: [feature request](https://github.com/neerajsunil/pip-recorder/issues/new?template=feature_request.yml).
- Vulnerabilities: [SECURITY.md](SECURITY.md).

Include the FastRecorder version (Settings → General), OS/build, CPU architecture, GPU/driver and selected codec. Explain what you clicked and what happened. Error details are available through the studio's Details button; technical settings are under Settings → Info. Fatal diagnostics are in `%LOCALAPPDATA%\FastRecorder\diagnostics.log`. Review screenshots, paths and logs before sharing them.

For a blank or glitchy UI, restart with `fastrecorder.exe --software-ui`. This changes only UI rendering; hardware video encoding remains available. Report whether this changes the behavior. For playback trouble, try a player with the chosen codec installed, or record H.264 for broader compatibility.

If a remembered audio device is disconnected, refresh or select another device under Settings → Audio, or disable that source. If Windows rejects a shortcut, choose an unused combination under Settings → Shortcuts and press Apply.
