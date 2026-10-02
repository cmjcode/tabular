import { Action, ActionPanel, closeMainWindow, Clipboard, Form, Icon, popToRoot, showToast, Toast } from "@raycast/api";
import { usePromise } from "@raycast/utils";
import { useState } from "react";
import { importUrl, openDeepLink, validateDsn } from "./lib/tabular";

interface DsnFormValues {
  dsn: string;
  name: string;
}

export default function OpenDsn() {
  const [dsnError, setDsnError] = useState<string | undefined>();

  // Isi awal dari clipboard bila isinya terlihat seperti DSN yang didukung.
  const { data: clipboardDsn } = usePromise(async () => {
    const text = (await Clipboard.readText())?.trim();
    return text && !validateDsn(text) ? text : undefined;
  });

  async function handleSubmit(values: DsnFormValues) {
    const dsn = values.dsn.trim();
    const error = validateDsn(dsn);
    if (error) {
      setDsnError(error);
      return;
    }
    try {
      await openDeepLink(importUrl(dsn, values.name.trim() || undefined));
      await closeMainWindow();
      await popToRoot({ clearSearchBar: true });
    } catch (e) {
      await showToast({
        style: Toast.Style.Failure,
        title: "Could not open Tabular",
        message: e instanceof Error ? e.message : String(e),
      });
    }
  }

  return (
    <Form
      actions={
        <ActionPanel>
          <Action.SubmitForm title="Open in Tabular" icon={Icon.AppWindow} onSubmit={handleSubmit} />
        </ActionPanel>
      }
    >
      <Form.Description text="Tabular opens its new-connection form prefilled with this connection string. Nothing is saved until you confirm in the app." />
      <Form.TextField
        id="dsn"
        key={clipboardDsn ?? "empty"}
        title="Connection String"
        placeholder="postgres://user:password@localhost:5432/app"
        defaultValue={clipboardDsn}
        error={dsnError}
        onChange={() => dsnError && setDsnError(undefined)}
        onBlur={(event) => {
          const value = event.target.value?.trim();
          setDsnError(value ? validateDsn(value) : undefined);
        }}
        info="Supported: postgres, postgresql, mysql, mariadb, sqlite:///abs/path, sqlserver, mssql, redis, rediss, mongodb, mongodb+srv"
        autoFocus
      />
      <Form.TextField id="name" title="Name" placeholder="Optional connection name" />
    </Form>
  );
}
