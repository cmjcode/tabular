# Platform integration

Deep links, CLI, OS automation, localization, updates, and privacy controls.

## `tabular://` links

| Link | Effect |
|---|---|
| `tabular://open?connection=<id or name>[&database=<db>][&table=<table>]` | Opens a saved connection in a new query tab, or opens the table. |
| `tabular://query?connection=<id or name>&sql=<percent-encoded>[&database=<db>][&run=1]` | Opens a new query tab containing the SQL. `run=1` shows a confirmation dialog first. A link never runs SQL on its own. |
| `tabular://import?url=<percent-encoded DSN>[&name=<name>]` | If a saved connection points at the same server and database, opens it. Otherwise opens the New Connection form, prefilled. |

- `connection` accepts the numeric id, the name, or `Folder/Name`. Names are case-insensitive.
- A raw DSN can be used anywhere a link is accepted. Supported schemes: `postgres://`, `postgresql://`, `mysql://`, `mariadb://`, `sqlite:///abs/path.db`, `sqlserver://` (also JDBC `;key=value` style), `redis://`, `rediss://`, `mongodb://`, `mongodb+srv://`.
- Links are capped at 256 KiB.
- If the app is still loading its connection list, a link is retried for up to 10 seconds.

How a link reaches the app:

- **macOS**: through an Apple Event, with the scheme registered in `Info.plist`. This covers both the `cargo bundle` and Xcode builds.
- **Linux**: through `tabular.desktop` (`MimeType=x-scheme-handler/tabular`, `Exec=tabular %u`). This also applies to the Flatpak build. After installing, run `xdg-mime default tabular.desktop x-scheme-handler/tabular` if another app has claimed the scheme.
- **Windows**: the MSI registers `HKLM\Software\Classes\tabular`.
- **Running instance**: a second process hands the URL to the running window, then exits. The hand-off uses `127.0.0.1` plus a random token stored in `<data dir>/instance.json`, which has `0600` permissions on Unix.
- **iOS/iPadOS**: the scheme is registered in `apple/ios/Info.plist`, and URLs are received by the app delegate. This means the Shortcuts **Open URLs** action can drive Tabular.

## CLI

```bash
tabular open "tabular://open?connection=Local"
tabular open postgres://app:secret@localhost:5432/app
tabular connections          # human-readable list
tabular connections --json   # id, name, type, host, port, database, folder, environment
```

`tabular connections` never prints passwords.

On macOS, when you run `open` from the binary inside the `.app`, it launches the bundle through LaunchServices instead of tying the GUI to the terminal.

### ddev

Copy `integrations/ddev/commands/host/tabular` into `.ddev/commands/host/`, or install it with `ddev add-on get`. Then run `ddev tabular` to open the project database. See `integrations/ddev/README.md`.

### Raycast

`integrations/raycast/` provides two commands:

- **Search Connections**: open a connection, start a new query, or copy a deep link.
- **Open Database URL**

To install, run `npm install && npm run dev` in that folder. Before publishing, set the `author` field in `package.json`.

## AppleScript (macOS)

The dictionary is in `apple/macos/Tabular.sdef` and is bundled as `Contents/Resources/Tabular.sdef`.

```applescript
tell application "Tabular"
    open connection "Local" database "app" table "users"
    new query "select count(*) from orders" connection "Prod" -- opens a tab, does not run
    open deep link "postgres://app@localhost/app"
    connection names
end tell
```

`open location "tabular://..."` also works.

## Handoff (macOS)

Handoff is off by default. Turn it on in **Preferences > Privacy > Handoff**.

When it is on, the active tab is advertised as activity type `id.tabular.database.query`, whose payload is a `tabular://` link:

- If the SQL is 2,000 characters or shorter, the link includes the SQL.
- If it is longer, the link only includes the connection.

The receiving Mac or iPad opens the link like any other link. Handoff only works when all of the following are true:

- the app is signed with a Team ID;
- `NSUserActivityTypes` is listed in `Info.plist`;
- both devices use the same Apple Account.

## Touch ID (macOS)

To set it up:

1. In **Preferences > Cloud Sync**, with the vault unlocked, turn on **Offer Touch ID to unlock the vault**.
2. The next time you unlock with your passphrase, it is stored in the macOS Keychain.
3. From then on, the unlock form shows **Unlock with Touch ID**.

Security note: the `keyring` crate cannot attach a biometric access-control policy. Touch ID therefore gates Tabular's own read of the Keychain item, but the item itself is protected by the login keychain. Turning the option off deletes the stored passphrase.

## Language

Choose the language in **Preferences > Appearance > Language**. Available languages are English, Bahasa Indonesia, 한국어, Türkçe, Tiếng Việt and 简体中文. The default follows the system language.

For Korean and Chinese, Tabular loads a system CJK font as a fallback:

- macOS: Apple SD Gothic Neo or Hiragino Sans GB
- Windows: Malgun Gothic or Microsoft YaHei
- Linux: Noto Sans CJK, WenQuanYi or Nanum

Coverage so far:

- Translated: Preferences navigation, the Updates, Privacy and Language pages, the update notification, the sidebar sections, deep-link dialogs, environment labels, and Touch ID.
- Still in English: the rest of the UI.

To add a string, wrap the literal in `crate::i18n::tr("...")` and add a row to `src/i18n/tables.rs`. A unit test checks that every language has the row and that the `{}` placeholders match.

## Updates

In **Preferences > Updates**:

- **Download updates in the background** is on by default. When it is off, you only get a notification.
- **Install when quitting**: on macOS, the staged `.app` replaces the installed one after Tabular exits, without relaunching. On Linux and Windows, the binary is already replaced during staging.
- **Skip This Version**: available from the update notification.

## Managed policy (MDM / fleet)

Tabular reads the first policy file it finds, in this order:

1. `$TABULAR_POLICY_FILE`
2. macOS: `/Library/Managed Preferences/<user>/id.tabular.database.plist`
3. macOS: `/Library/Managed Preferences/id.tabular.database.plist` (a configuration profile with payload domain `id.tabular.database`)
4. macOS: `/Library/Application Support/Tabular/policy.json`
5. Linux: `/etc/tabular/policy.json`
6. Windows: `%ProgramData%\Tabular\policy.json`

Example policy:

```json
{
  "disableUpdateCheck": false,
  "automaticDownload": true,
  "installOnQuit": true,
  "minimumVersion": "1.1.0",
  "disabledNetworkCategories": ["ai", "map_tiles"],
  "language": "id"
}
```

Any value you set overrides the user's choice, and the matching control is disabled in the UI with the note "Managed by your organization".

## Privacy

**Preferences > Privacy** lists every connection Tabular makes outside your databases. Each category has a toggle, except where noted:

| Key | What | Default |
|---|---|---|
| `update_check` | GitHub releases API | on |
| `update_download` | Release asset download | on |
| `cloud_sync` | Sync server, only while signed in (sign-in, vault, teams) | on |
| `ai` | Configured AI provider (API backend) | on |
| `map_tiles` | OpenStreetMap tiles for the Map view | on |
| `handoff` | Apple Handoff (macOS only) | off |

The page also lists the connections that you always start yourself and that have no toggle: database servers, HTTP client requests, CLI agents, and links opened in the browser.

A blocked request fails with a message that names the setting responsible. The page shows a log of the last 200 requests in the current session, recording only host and path; query strings and credentials are removed.

## Connection environments

Right-click a connection and choose **Environment** to set Production, Staging, Development, Testing or Local.

- The environment shows as a colored strip on query tabs, as a badge next to the connection picker, and in the deep-link Run confirmation.
- Without an explicit value, the environment is guessed from words in the connection name, for example `prod`, `staging`/`uat`/`preprod`, `dev`, `qa`/`test`, `local`.
- The value is stored in the local `connection_environment` table and is not synced.

## iPad layout

When the window is narrower than 700 pt (Slide Over, or Split View at 1/3 or 1/2), Tabular hides the sidebar and the AI panel. They come back when the window is wide enough again.

The sidebar and the Preferences dialog are limited to the window width. You can still open the sidebar by hand while the window is narrow.

## iPad files and keyboard

- **Open / Import** uses the system document picker (Files, iCloud Drive, external drives). Picked files are copied into the app sandbox, so the original is never modified. Folder pickers (for example a Git repository or a data directory) keep access to the chosen folder for the rest of the session.
- **Save / Export** does not show a picker. The file is written to **Files › On My iPad › Tabular** under the name shown in the dialog; an existing name gets a ` (2)`, ` (3)` suffix. From there you can share, move or AirDrop it with the Files app.
- The on-screen keyboard supports autocorrect, predictive text, dictation and CJK composition in the SQL editor. The keyboard avoids the caret, so the line you are typing stays visible.

## Accessibility

Tabular renders its own UI (egui). On macOS, Windows and Linux the UI tree is exposed to screen readers through AccessKit. **iPadOS and Android are not covered yet**: VoiceOver and TalkBack do not see individual controls, only the window. AccessKit has no UIKit/Android backend at the time of writing; this will be enabled as soon as it exists upstream. Dynamic Type is approximated by the **Touch / tablet** UI mode in Preferences, which enlarges fonts and hit targets.
