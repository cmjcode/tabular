import { getPreferenceValues, open } from "@raycast/api";
import { execFile } from "node:child_process";
import { accessSync, constants } from "node:fs";
import { homedir } from "node:os";
import { delimiter, join } from "node:path";
import { promisify } from "node:util";

const execFileAsync = promisify(execFile);

const MACOS_APP_BINARY = "/Applications/Tabular.app/Contents/MacOS/tabular";

export type ConnectionType = "MySQL" | "PostgreSQL" | "SQLite" | "Redis" | "MsSQL" | "MongoDB" | "ApiHttp";

export interface TabularConnection {
  id: number;
  name: string;
  type: ConnectionType | string;
  host: string;
  port: string;
  database: string;
  folder: string | null;
  environment: string | null;
}

interface TabularPreferences {
  tabularPath?: string;
}

/** Dilempar bila binary `tabular` tidak ditemukan di lokasi mana pun. */
export class TabularNotFoundError extends Error {
  constructor(message = "Tabular was not found. Install Tabular or set the binary path in the extension preferences.") {
    super(message);
    this.name = "TabularNotFoundError";
  }
}

/** Dilempar bila CLI `tabular` berjalan tetapi gagal (exit code non-zero). */
export class TabularCommandError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "TabularCommandError";
  }
}

function isExecutable(path: string): boolean {
  try {
    accessSync(path, process.platform === "win32" ? constants.F_OK : constants.X_OK);
    return true;
  } catch {
    return false;
  }
}

/**
 * Urutan pencarian: preferensi -> env TABULAR_BIN -> Tabular.app (macOS) -> PATH.
 * PATH milik Raycast sangat minim, jadi direktori install umum ikut diperiksa.
 */
export function resolveTabularBinary(): string {
  const { tabularPath } = getPreferenceValues<TabularPreferences>();
  const explicit = tabularPath?.trim() || process.env.TABULAR_BIN?.trim();
  if (explicit) {
    if (!isExecutable(explicit)) {
      throw new TabularNotFoundError(`Tabular binary not found or not executable: ${explicit}`);
    }
    return explicit;
  }

  if (process.platform === "darwin" && isExecutable(MACOS_APP_BINARY)) {
    return MACOS_APP_BINARY;
  }

  const exeName = process.platform === "win32" ? "tabular.exe" : "tabular";
  const dirs = [
    ...(process.env.PATH ?? "").split(delimiter),
    "/opt/homebrew/bin",
    "/usr/local/bin",
    "/usr/bin",
    join(homedir(), ".cargo", "bin"),
    join(homedir(), ".local", "bin"),
  ].filter(Boolean);
  for (const dir of dirs) {
    const candidate = join(dir, exeName);
    if (isExecutable(candidate)) {
      return candidate;
    }
  }
  throw new TabularNotFoundError();
}

/** Menjalankan CLI dengan array argumen (tanpa shell) dan mengembalikan stdout. */
export async function runTabular(args: string[], timeoutMs = 15_000): Promise<string> {
  const bin = resolveTabularBinary();
  try {
    const { stdout } = await execFileAsync(bin, args, {
      timeout: timeoutMs,
      maxBuffer: 16 * 1024 * 1024,
      windowsHide: true,
    });
    return stdout;
  } catch (error) {
    const err = error as NodeJS.ErrnoException & { stderr?: string };
    if (err.code === "ENOENT") {
      throw new TabularNotFoundError(`Tabular binary not found: ${bin}`);
    }
    const detail = err.stderr?.trim() || err.message;
    throw new TabularCommandError(`tabular ${args[0] ?? ""} failed: ${detail}`);
  }
}

export async function listConnections(): Promise<TabularConnection[]> {
  const stdout = await runTabular(["connections", "--json"]);
  let parsed: unknown;
  try {
    parsed = JSON.parse(stdout);
  } catch {
    throw new TabularCommandError("tabular connections --json returned invalid JSON.");
  }
  if (!Array.isArray(parsed)) {
    throw new TabularCommandError("tabular connections --json did not return an array.");
  }
  return parsed.map((raw) => {
    const c = raw as Partial<TabularConnection>;
    return {
      id: Number(c.id),
      name: String(c.name ?? ""),
      type: String(c.type ?? ""),
      host: String(c.host ?? ""),
      port: c.port == null ? "" : String(c.port),
      database: String(c.database ?? ""),
      folder: c.folder ? String(c.folder) : null,
      environment: c.environment ? String(c.environment) : null,
    };
  });
}

/** Parameter kosong/undefined dilewati; nilai di-encode dengan encodeURIComponent (spasi jadi %20, bukan +). */
function buildDeepLink(action: "open" | "query" | "import", params: Record<string, string | undefined>): string {
  const query = Object.entries(params)
    .filter((entry): entry is [string, string] => entry[1] !== undefined && entry[1] !== "")
    .map(([key, value]) => `${encodeURIComponent(key)}=${encodeURIComponent(value)}`)
    .join("&");
  return `tabular://${action}?${query}`;
}

export function openConnectionUrl(connection: string | number, options: { database?: string; table?: string } = {}) {
  return buildDeepLink("open", {
    connection: String(connection),
    database: options.database,
    table: options.table,
  });
}

export function queryUrl(connection: string | number, sql: string, options: { database?: string; run?: boolean } = {}) {
  return buildDeepLink("query", {
    connection: String(connection),
    sql,
    database: options.database,
    run: options.run ? "1" : undefined,
  });
}

export function importUrl(dsn: string, name?: string) {
  return buildDeepLink("import", { url: dsn, name });
}

export const SUPPORTED_DSN_SCHEMES = [
  "postgres",
  "postgresql",
  "mysql",
  "mariadb",
  "sqlite",
  "sqlserver",
  "mssql",
  "redis",
  "rediss",
  "mongodb",
  "mongodb+srv",
];

/** Mengembalikan pesan error untuk DSN yang tidak didukung, atau undefined bila valid. */
export function validateDsn(dsn: string): string | undefined {
  const trimmed = dsn.trim();
  const match = /^([a-z][a-z0-9+.-]*):\/\//i.exec(trimmed);
  if (!match) {
    return "Expected a URL such as postgres://user@host:5432/db";
  }
  const scheme = match[1].toLowerCase();
  if (!SUPPORTED_DSN_SCHEMES.includes(scheme)) {
    return `Unsupported scheme "${scheme}://"`;
  }
  if (scheme === "sqlite" && !trimmed.toLowerCase().startsWith("sqlite:///")) {
    return "SQLite DSNs need an absolute path: sqlite:///abs/path.db";
  }
  return undefined;
}

/**
 * Membuka deep link lewat handler URL sistem. Bila skema `tabular://` belum
 * terdaftar, fallback ke `tabular open <url>` yang meneruskan ke instance aktif.
 */
export async function openDeepLink(url: string): Promise<void> {
  try {
    await open(url);
  } catch {
    await runTabular(["open", url], 30_000);
  }
}
