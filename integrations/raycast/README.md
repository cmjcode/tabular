# Tabular for Raycast

Raycast commands for [Tabular](https://github.com/tabular-id/tabular). The extension talks to
Tabular only through its CLI (`tabular connections --json`, `tabular open <url>`) and the
`tabular://` URL scheme, so it never sees passwords.

## Commands

| Command | What it does |
|---|---|
| **Search Connections** | Lists saved connections grouped by folder, searchable by name, host, database and type. Actions: *Open in Tabular*, *New Query* (SQL editor form, opens the query in Tabular without running it), *Copy Deep Link*, *Refresh*. |
| **Open Connection String** | Paste a DSN (`postgres://`, `mysql://`, `sqlite:///abs/path`, `mongodb+srv://`, ...) and an optional name. Tabular opens its new-connection form prefilled; nothing is saved until you confirm. Prefills from the clipboard when it holds a DSN. |

## Install (development)

```bash
cd integrations/raycast
npm install && npm run dev
```

`npm run dev` runs `ray develop`, which imports the extension into Raycast and hot-reloads it.

Other scripts: `npm run typecheck` (`tsc --noEmit`), `npm run lint` (`ray lint`), `npm run build`.

## Finding the Tabular binary

In order:

1. The **Tabular Binary** extension preference.
2. The `TABULAR_BIN` environment variable (only visible if Raycast itself was launched with it).
3. `/Applications/Tabular.app/Contents/MacOS/tabular` on macOS.
4. `tabular` on `PATH`, plus `/opt/homebrew/bin`, `/usr/local/bin`, `~/.cargo/bin` and
   `~/.local/bin` (Raycast's own `PATH` is minimal).

Deep links are opened through the system URL handler. If the `tabular://` scheme is not
registered, the extension falls back to `tabular open <url>`.

## Publishing

`author` in `package.json` must be a real Raycast Store username before `ray lint` / `npm run
publish` will pass. `assets/extension-icon.png` is Tabular's logo at 512x512.
