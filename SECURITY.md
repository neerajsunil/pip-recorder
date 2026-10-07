# Security policy

FastRecorder is a screen/audio recorder. It handles potentially sensitive local media and must not upload recordings, capture an enabled source secretly, load arbitrary driver DLLs, or overwrite existing recordings.

## Supported versions

During the preview phase, fixes target the latest preview and the main branch. Older previews are not maintained separately. No stable production release has been declared yet.

## Report a vulnerability privately

Use [GitHub private vulnerability reporting](https://github.com/neerajsunil/pip-recorder/security/advisories/new). Include the affected version, Windows build, relevant GPU/driver, reproduction steps and expected impact. Do not post credentials, private recordings or sensitive desktop screenshots. If the private form is unavailable, open an issue asking the maintainer for a private contact without disclosing exploit details.

Please avoid a public exploit report until the maintainer has had a reasonable opportunity to investigate and coordinate a fix. Response and patch timelines depend on maintainer availability; no guaranteed response SLA is offered.

## Privacy

FastRecorder has no telemetry, account system, uploads or background service. Preferences are stored in `%LOCALAPPDATA%\FastRecorder`; videos are saved to the folder you choose. Fatal diagnostic logs, if produced, are local and may contain paths/error details. Only share logs after reviewing them. App-selected windows can contain private information; review a recording before distributing it.
