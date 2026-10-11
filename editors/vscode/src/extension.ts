// A minimal VS Code client for the Kanagawa language server.
//
// It does one job: spawn the server and get out of the way. Everything a user
// sees — the outline, folding, semantic highlighting, diagnostics — is computed
// in `crates/kg-lsp`, so nothing about the language is duplicated here. That is
// deliberate: the reference extension is a TextMate grammar and nothing else,
// and a second implementation of the language in TypeScript is exactly the
// thing that makes editor support drift from the compiler.
//
// The bundled TextMate grammar is a *fallback*, used only when the server is
// not installed or has crashed. See README.md.

import * as vscode from "vscode";
import {
  LanguageClient,
  LanguageClientOptions,
  ServerOptions,
  TransportKind,
} from "vscode-languageclient/node";

let client: LanguageClient | undefined;
let log: vscode.OutputChannel;

export async function activate(context: vscode.ExtensionContext): Promise<void> {
  log = vscode.window.createOutputChannel("Kanagawa Language Server");
  context.subscriptions.push(log);

  context.subscriptions.push(
    vscode.commands.registerCommand("kanagawa.restartServer", async () => {
      await stop();
      await start(context);
    }),
    vscode.commands.registerCommand("kanagawa.showServerLog", () => log.show()),
  );

  // A change to how the server is launched is only actionable by relaunching it.
  context.subscriptions.push(
    vscode.workspace.onDidChangeConfiguration(async (event) => {
      if (
        event.affectsConfiguration("kanagawa.server.path") ||
        event.affectsConfiguration("kanagawa.server.args") ||
        event.affectsConfiguration("kanagawa.server.extraEnv")
      ) {
        await stop();
        await start(context);
      }
    }),
  );

  await start(context);
}

export async function deactivate(): Promise<void> {
  await stop();
}

async function start(context: vscode.ExtensionContext): Promise<void> {
  const config = vscode.workspace.getConfiguration("kanagawa");
  const command = config.get<string>("server.path", "kg");
  const args = config.get<string[]>("server.args", ["lsp"]);
  const extraEnv = config.get<Record<string, string>>("server.extraEnv", {});

  const run = {
    command,
    args,
    transport: TransportKind.stdio,
    options: { env: { ...process.env, ...extraEnv } },
  };

  const serverOptions: ServerOptions = { run, debug: run };

  const clientOptions: LanguageClientOptions = {
    documentSelector: [{ scheme: "file", language: "kanagawa" }],
    outputChannel: log,
    // The server discovers `kanagawa.toml` itself, walking up from each file,
    // but it is told when the manifest changes, so that editing import
    // directories takes effect without a restart, and when a source changes
    // on disk — a branch switch, a pull — so that an import of it is read
    // again rather than served stale.
    synchronize: {
      fileEvents: [
        vscode.workspace.createFileSystemWatcher("**/kanagawa.toml"),
        vscode.workspace.createFileSystemWatcher("**/*.k"),
      ],
    },
    // Without this a failed spawn pops a modal on every keystroke.
    initializationFailedHandler: (error) => {
      log.appendLine(`failed to start ${command}: ${error}`);
      void vscode.window.showWarningMessage(
        `Kanagawa: could not start "${command}". Syntax highlighting will fall back ` +
          `to the bundled TextMate grammar. Set "kanagawa.server.path" to your ` +
          `kg (or kg-lsp) binary.`,
        "Open Settings",
      ).then((choice) => {
        if (choice === "Open Settings") {
          void vscode.commands.executeCommand(
            "workbench.action.openSettings",
            "kanagawa.server.path",
          );
        }
      });
      return false;
    },
  };

  client = new LanguageClient(
    "kanagawa",
    "Kanagawa Language Server",
    serverOptions,
    clientOptions,
  );

  try {
    await client.start();
    context.subscriptions.push(client);
  } catch (error) {
    log.appendLine(`failed to start ${command} ${args.join(" ")}: ${error}`);
    client = undefined;
  }
}

async function stop(): Promise<void> {
  if (!client) {
    return;
  }
  const stopping = client;
  client = undefined;
  await stopping.stop();
}
