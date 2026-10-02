import {
  Action,
  ActionPanel,
  closeMainWindow,
  Color,
  Form,
  Icon,
  Keyboard,
  List,
  openExtensionPreferences,
  popToRoot,
  showToast,
  Toast,
} from "@raycast/api";
import { usePromise } from "@raycast/utils";
import { useMemo } from "react";
import {
  listConnections,
  openConnectionUrl,
  openDeepLink,
  queryUrl,
  TabularConnection,
  TabularNotFoundError,
} from "./lib/tabular";

const NO_FOLDER = "No Folder";

function environmentColor(environment: string): Color {
  switch (environment.toLowerCase()) {
    case "production":
    case "prod":
      return Color.Red;
    case "staging":
      return Color.Orange;
    case "development":
    case "dev":
    case "local":
      return Color.Green;
    default:
      return Color.SecondaryText;
  }
}

function subtitle(connection: TabularConnection): string {
  const hostPort = connection.port ? `${connection.host}:${connection.port}` : connection.host;
  return [hostPort, connection.database].filter(Boolean).join(" / ");
}

async function openAndClose(url: string, failureTitle: string) {
  try {
    await openDeepLink(url);
    await closeMainWindow();
    await popToRoot({ clearSearchBar: true });
  } catch (error) {
    await showToast({
      style: Toast.Style.Failure,
      title: failureTitle,
      message: error instanceof Error ? error.message : String(error),
    });
  }
}

function ConnectionActions({ connection, onRefresh }: { connection: TabularConnection; onRefresh: () => void }) {
  const url = openConnectionUrl(connection.id);
  return (
    <ActionPanel>
      <Action
        title="Open in Tabular"
        icon={Icon.AppWindow}
        onAction={() => openAndClose(url, "Could not open Tabular")}
      />
      <Action.Push
        title="New Query"
        icon={Icon.Code}
        shortcut={Keyboard.Shortcut.Common.New}
        target={<NewQueryForm connection={connection} />}
      />
      <Action.CopyToClipboard title="Copy Deep Link" content={url} shortcut={Keyboard.Shortcut.Common.CopyDeeplink} />
      <Action
        title="Refresh"
        icon={Icon.ArrowClockwise}
        shortcut={Keyboard.Shortcut.Common.Refresh}
        onAction={onRefresh}
      />
    </ActionPanel>
  );
}

interface QueryFormValues {
  sql: string;
  database: string;
}

function NewQueryForm({ connection }: { connection: TabularConnection }) {
  async function handleSubmit(values: QueryFormValues) {
    const sql = values.sql.trim();
    if (!sql) {
      await showToast({ style: Toast.Style.Failure, title: "SQL is empty" });
      return;
    }
    // run sengaja tidak di-set: pengguna mengeksekusi sendiri di aplikasi.
    const url = queryUrl(connection.id, sql, { database: values.database.trim() || undefined });
    await openAndClose(url, "Could not send query to Tabular");
  }

  return (
    <Form
      navigationTitle={`New Query — ${connection.name}`}
      actions={
        <ActionPanel>
          <Action.SubmitForm title="Open Query in Tabular" icon={Icon.Code} onSubmit={handleSubmit} />
        </ActionPanel>
      }
    >
      <Form.Description title="Connection" text={`${connection.name} (${connection.type})`} />
      <Form.TextArea id="sql" title="SQL" placeholder="SELECT * FROM ..." enableMarkdown={false} autoFocus />
      <Form.TextField
        id="database"
        title="Database"
        placeholder="Optional"
        defaultValue={connection.database}
        info="Leave empty to use the connection's default database."
      />
    </Form>
  );
}

export default function SearchConnections() {
  const { data, isLoading, error, revalidate } = usePromise(listConnections, [], {
    failureToastOptions: { title: "Could not load Tabular connections" },
  });

  // Kelompokkan per folder; koneksi tanpa folder ditaruh paling akhir.
  const sections = useMemo(() => {
    const groups = new Map<string, TabularConnection[]>();
    for (const connection of data ?? []) {
      const key = connection.folder || NO_FOLDER;
      const list = groups.get(key) ?? [];
      list.push(connection);
      groups.set(key, list);
    }
    return [...groups.entries()].sort(([a], [b]) => {
      if (a === NO_FOLDER) return 1;
      if (b === NO_FOLDER) return -1;
      return a.localeCompare(b);
    });
  }, [data]);

  if (error && !isLoading) {
    const notFound = error instanceof TabularNotFoundError;
    return (
      <List>
        <List.EmptyView
          icon={notFound ? Icon.QuestionMarkCircle : Icon.Warning}
          title={notFound ? "Tabular Not Found" : "Could Not Load Connections"}
          description={error.message}
          actions={
            <ActionPanel>
              {notFound ? (
                <Action title="Open Extension Preferences" icon={Icon.Gear} onAction={openExtensionPreferences} />
              ) : (
                <Action title="Retry" icon={Icon.ArrowClockwise} onAction={revalidate} />
              )}
              <Action.OpenInBrowser title="Tabular Website" url="https://github.com/tabular-id/tabular" />
            </ActionPanel>
          }
        />
      </List>
    );
  }

  return (
    <List isLoading={isLoading} searchBarPlaceholder="Search connections by name, host or database">
      <List.EmptyView
        icon={Icon.HardDrive}
        title="No Connections"
        description="Add a connection in Tabular, or use the Open Connection String command."
      />
      {sections.map(([folder, connections]) => (
        <List.Section key={folder} title={folder} subtitle={String(connections.length)}>
          {connections.map((connection) => (
            <List.Item
              key={connection.id}
              icon={Icon.HardDrive}
              title={connection.name}
              subtitle={subtitle(connection)}
              keywords={[connection.host, connection.database, connection.type].filter(Boolean)}
              accessories={[
                ...(connection.environment
                  ? [{ tag: { value: connection.environment, color: environmentColor(connection.environment) } }]
                  : []),
                { tag: connection.type },
              ]}
              actions={<ConnectionActions connection={connection} onRefresh={revalidate} />}
            />
          ))}
        </List.Section>
      ))}
    </List>
  );
}
