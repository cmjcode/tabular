# ddev-tabular

A [ddev](https://ddev.com) host command that opens the current project's database in
[Tabular](https://github.com/tabular-id/tabular).

```bash
ddev tabular
```

It reads the database type, published host port and credentials from `ddev describe -j`
(when `jq` is installed), falling back to `DDEV_HOST_DB_PORT` / `DDEV_DATABASE_FAMILY` /
`DDEV_DATABASE` and ddev's defaults (`db` / `db` / `db` on `127.0.0.1`). It then runs:

```bash
tabular open "mysql://db:db@127.0.0.1:<port>/db"      # mysql, mariadb
tabular open "postgres://db:db@127.0.0.1:<port>/db"   # postgres
```

Tabular opens its new-connection form prefilled with that DSN (or forwards it to an
already-running instance).

## Install

As an add-on, from a local checkout or from the repository:

```bash
ddev add-on get /path/to/tabular/integrations/ddev
# or, once published as its own repo:
ddev add-on get <owner>/ddev-tabular
```

Or copy the command manually:

```bash
mkdir -p .ddev/commands/host
cp /path/to/tabular/integrations/ddev/commands/host/tabular .ddev/commands/host/
chmod +x .ddev/commands/host/tabular
```

The project must be running (`ddev start`) so the database port is published.

## Finding the Tabular binary

1. `TABULAR_BIN` environment variable, e.g. `TABULAR_BIN=~/.cargo/bin/tabular ddev tabular`.
2. `/Applications/Tabular.app/Contents/MacOS/tabular` on macOS.
3. `tabular` on `PATH` (`tabular.exe` on Windows under Git Bash).

ddev only shows the command when one of the paths in the `## HostBinaryExists:` header exists
(`/Applications/Tabular.app`, `/opt/homebrew/bin/tabular`, `/usr/local/bin/tabular`,
`/usr/bin/tabular`). If Tabular lives elsewhere (for example `~/.cargo/bin`), delete that header
line from `.ddev/commands/host/tabular` and set `TABULAR_BIN` if it is not on `PATH`.

Set `TABULAR_DB_HOST` to override `127.0.0.1` (for example with a remote Docker host).
