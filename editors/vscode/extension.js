'use strict';

const vscode = require('vscode');
const { spawn } = require('child_process');

const LANGUAGE_ID = 'knr';
const MAX_CONTENT_LENGTH = 32 * 1024 * 1024;
const REQUEST_TIMEOUT_MS = 10000;
const SHUTDOWN_TIMEOUT_MS = 2000;
const MAX_RESTARTS_PER_WINDOW = 3;
const RESTART_WINDOW_MS = 60000;

const COMPLETION_KIND_MAP = {
  1: vscode.CompletionItemKind.Text,
  2: vscode.CompletionItemKind.Method,
  3: vscode.CompletionItemKind.Function,
  4: vscode.CompletionItemKind.Constructor,
  5: vscode.CompletionItemKind.Field,
  6: vscode.CompletionItemKind.Variable,
  7: vscode.CompletionItemKind.Class,
  8: vscode.CompletionItemKind.Interface,
  9: vscode.CompletionItemKind.Module,
  10: vscode.CompletionItemKind.Property,
  11: vscode.CompletionItemKind.Unit,
  12: vscode.CompletionItemKind.Value,
  13: vscode.CompletionItemKind.Enum,
  14: vscode.CompletionItemKind.Keyword,
  15: vscode.CompletionItemKind.Snippet,
  16: vscode.CompletionItemKind.Color,
  17: vscode.CompletionItemKind.File,
  18: vscode.CompletionItemKind.Reference,
  19: vscode.CompletionItemKind.Folder,
  20: vscode.CompletionItemKind.EnumMember,
  21: vscode.CompletionItemKind.Constant,
  22: vscode.CompletionItemKind.Struct,
  23: vscode.CompletionItemKind.Event,
  24: vscode.CompletionItemKind.Operator,
  25: vscode.CompletionItemKind.TypeParameter,
};

const SYMBOL_KIND_MAP = {
  1: vscode.SymbolKind.File,
  2: vscode.SymbolKind.Module,
  3: vscode.SymbolKind.Namespace,
  4: vscode.SymbolKind.Package,
  5: vscode.SymbolKind.Class,
  6: vscode.SymbolKind.Method,
  7: vscode.SymbolKind.Property,
  8: vscode.SymbolKind.Field,
  9: vscode.SymbolKind.Constructor,
  10: vscode.SymbolKind.Enum,
  11: vscode.SymbolKind.Interface,
  12: vscode.SymbolKind.Function,
  13: vscode.SymbolKind.Variable,
  14: vscode.SymbolKind.Constant,
  15: vscode.SymbolKind.String,
  16: vscode.SymbolKind.Number,
  17: vscode.SymbolKind.Boolean,
  18: vscode.SymbolKind.Array,
  19: vscode.SymbolKind.Object,
  20: vscode.SymbolKind.Key,
  21: vscode.SymbolKind.Null,
  22: vscode.SymbolKind.EnumMember,
  23: vscode.SymbolKind.Struct,
  24: vscode.SymbolKind.Event,
  25: vscode.SymbolKind.Operator,
  26: vscode.SymbolKind.TypeParameter,
};

const SEVERITY_MAP = {
  1: vscode.DiagnosticSeverity.Error,
  2: vscode.DiagnosticSeverity.Warning,
  3: vscode.DiagnosticSeverity.Information,
  4: vscode.DiagnosticSeverity.Hint,
};

function toVscodeRange(range) {
  if (!range || !range.start || !range.end) {
    return new vscode.Range(0, 0, 0, 0);
  }
  const startLine = Math.max(0, range.start.line | 0);
  const startChar = Math.max(0, range.start.character | 0);
  const endLine = Math.max(startLine, range.end.line | 0);
  const endChar = Math.max(0, range.end.character | 0);
  return new vscode.Range(startLine, startChar, endLine, endChar);
}

function toHoverContents(contents) {
  if (!contents) {
    return null;
  }
  if (typeof contents === 'string') {
    return new vscode.MarkdownString(contents);
  }
  if (Array.isArray(contents)) {
    return contents.map(toHoverContents).filter(Boolean);
  }
  if (typeof contents === 'object' && typeof contents.value === 'string') {
    if (contents.kind === 'markdown') {
      return new vscode.MarkdownString(contents.value);
    }
    if (typeof contents.language === 'string') {
      const md = new vscode.MarkdownString();
      md.appendCodeblock(contents.value, contents.language);
      return md;
    }
    return contents.value;
  }
  return null;
}

function toCompletionItem(item) {
  const label = typeof item.label === 'string' ? item.label : String(item.label || '');
  const kind = COMPLETION_KIND_MAP[item.kind] ?? vscode.CompletionItemKind.Text;
  const ci = new vscode.CompletionItem(label, kind);

  if (typeof item.detail === 'string') {
    ci.detail = item.detail;
  }
  if (typeof item.sortText === 'string') {
    ci.sortText = item.sortText;
  }
  if (typeof item.filterText === 'string') {
    ci.filterText = item.filterText;
  }
  if (typeof item.documentation === 'string') {
    ci.documentation = item.documentation;
  } else if (item.documentation && typeof item.documentation.value === 'string') {
    ci.documentation =
      item.documentation.kind === 'markdown'
        ? new vscode.MarkdownString(item.documentation.value)
        : item.documentation.value;
  }
  if (typeof item.insertText === 'string') {
    ci.insertText =
      item.insertTextFormat === 2
        ? new vscode.SnippetString(item.insertText)
        : item.insertText;
  }
  return ci;
}

function toDocumentSymbol(sym) {
  const kind = SYMBOL_KIND_MAP[sym.kind] ?? vscode.SymbolKind.Function;
  const range = toVscodeRange(sym.range);
  const selectionRange = toVscodeRange(sym.selectionRange || sym.range);
  const docSym = new vscode.DocumentSymbol(
    sym.name || '<anonymous>',
    sym.detail || '',
    kind,
    range,
    selectionRange
  );
  if (Array.isArray(sym.children) && sym.children.length > 0) {
    docSym.children = sym.children.map(toDocumentSymbol);
  }
  return docSym;
}

class KnrLspClient {
  constructor(output, diagnostics) {
    this.output = output;
    this.diagnostics = diagnostics;
    this.proc = null;
    this.rxBuffer = Buffer.alloc(0);
    this.nextId = 1;
    this.pending = new Map();
    this.isReady = false;
    this.isStopping = false;
    this.startPromise = null;
    this.restartTimestamps = [];
    this.missingBinaryNotified = false;
  }

  getConfig() {
    const cfg = vscode.workspace.getConfiguration('knr');
    const serverPath = cfg.get('serverPath', 'knr') || 'knr';
    const rawArgs = cfg.get('serverArgs', ['lsp']);
    const serverArgs = Array.isArray(rawArgs) ? rawArgs.map(String) : ['lsp'];
    const trace = cfg.get('trace.server', 'off');
    return { serverPath, serverArgs, trace };
  }

  traceLog(direction, message) {
    const { trace } = this.getConfig();
    if (trace === 'off') {
      return;
    }
    if (trace === 'verbose') {
      this.output.appendLine(`[${direction}] ${JSON.stringify(message)}`);
    } else {
      const label = message.method
        ? `${message.method}${message.id !== undefined ? ` (#${message.id})` : ''}`
        : `response #${message.id}`;
      this.output.appendLine(`[${direction}] ${label}`);
    }
  }

  async ensureStarted() {
    if (this.isReady && this.proc && !this.proc.killed) {
      return;
    }
    if (this.startPromise) {
      return this.startPromise;
    }
    this.startPromise = this.startInternal().finally(() => {
      this.startPromise = null;
    });
    return this.startPromise;
  }

  async startInternal() {
    const { serverPath, serverArgs } = this.getConfig();
    const folders = vscode.workspace.workspaceFolders || [];
    const cwd = folders.length > 0 ? folders[0].uri.fsPath : undefined;

    this.isStopping = false;
    this.isReady = false;
    this.rxBuffer = Buffer.alloc(0);

    this.output.appendLine(`[knr-lsp] Starting: ${serverPath} ${serverArgs.join(' ')}`);

    let child;
    try {
      child = spawn(serverPath, serverArgs, {
        cwd,
        stdio: ['pipe', 'pipe', 'pipe'],
      });
    } catch (err) {
      this.handleSpawnError(err, serverPath);
      throw err;
    }

    this.proc = child;

    child.stdout.on('data', (chunk) => {
      if (this.proc === child) {
        this.handleStdoutChunk(chunk);
      }
    });

    child.stderr.on('data', (chunk) => {
      const text = chunk.toString('utf8').trimEnd();
      if (text) {
        this.output.appendLine(`[knr-lsp stderr] ${text}`);
      }
    });

    child.on('error', (err) => {
      if (this.proc !== child) {
        return;
      }
      this.handleSpawnError(err, serverPath);
      this.cleanupProcessState(new Error(`knr-lsp process error: ${err.message}`));
    });

    child.on('exit', (code, signal) => {
      if (this.proc !== child) {
        return;
      }
      const wasStopping = this.isStopping;
      this.output.appendLine(
        `[knr-lsp] Exited (code=${code === null ? 'null' : code}, signal=${signal || 'none'})`
      );
      this.cleanupProcessState(new Error(`knr-lsp exited (code=${code}, signal=${signal})`));

      if (!wasStopping) {
        this.scheduleAutoRestart();
      }
    });

    const initParams = {
      processId: process.pid,
      clientInfo: {
        name: 'knr-vscode',
        version: '0.1.0',
      },
      rootUri: folders.length > 0 ? folders[0].uri.toString() : null,
      workspaceFolders: folders.map((f) => ({
        uri: f.uri.toString(),
        name: f.name,
      })),
      capabilities: {
        textDocument: {
          synchronization: {
            didSave: true,
            dynamicRegistration: false,
            willSave: false,
            willSaveWaitUntil: false,
          },
          hover: {
            contentFormat: ['markdown', 'plaintext'],
          },
          completion: {
            completionItem: {
              snippetSupport: true,
              documentationFormat: ['markdown', 'plaintext'],
            },
          },
          documentSymbol: {
            hierarchicalDocumentSymbolSupport: true,
          },
          publishDiagnostics: {},
        },
      },
    };

    const result = await this.sendRawRequest('initialize', initParams, REQUEST_TIMEOUT_MS);
    const serverInfo = result && result.serverInfo ? result.serverInfo : {};
    this.output.appendLine(
      `[knr-lsp] Initialized ${serverInfo.name || 'knr-lsp'} ${serverInfo.version || ''}`.trim()
    );

    this.sendRawNotification('initialized', {});
    this.isReady = true;
    this.missingBinaryNotified = false;
    this.syncOpenDocuments();
  }

  handleSpawnError(err, serverPath) {
    this.output.appendLine(`[knr-lsp] Failed to start "${serverPath}": ${err.message}`);
    if (err && err.code === 'ENOENT' && !this.missingBinaryNotified) {
      this.missingBinaryNotified = true;
      vscode.window
        .showErrorMessage(
          `Could not launch knr Language Server ("${serverPath}"). Install the \`knr\` CLI on your PATH or configure "knr.serverPath".`,
          'Open Settings'
        )
        .then((choice) => {
          if (choice === 'Open Settings') {
            vscode.commands.executeCommand('workbench.action.openSettings', 'knr.serverPath');
          }
        });
    }
  }

  scheduleAutoRestart() {
    const now = Date.now();
    this.restartTimestamps = this.restartTimestamps.filter((t) => now - t < RESTART_WINDOW_MS);
    if (this.restartTimestamps.length >= MAX_RESTARTS_PER_WINDOW) {
      this.output.appendLine(
        '[knr-lsp] Server crashed repeatedly; automatic restart paused. Run "knr: Restart Language Server" to retry.'
      );
      return;
    }
    this.restartTimestamps.push(now);
    setTimeout(() => {
      if (!this.isStopping && !this.proc) {
        const hasOpenKnr = vscode.workspace.textDocuments.some(
          (doc) => doc.languageId === LANGUAGE_ID
        );
        if (hasOpenKnr) {
          this.ensureStarted().catch(() => {});
        }
      }
    }, 500);
  }

  cleanupProcessState(reason) {
    this.proc = null;
    this.isReady = false;
    this.rxBuffer = Buffer.alloc(0);
    for (const entry of this.pending.values()) {
      clearTimeout(entry.timer);
      entry.reject(reason);
    }
    this.pending.clear();
  }

  async stop() {
    this.isStopping = true;
    const child = this.proc;
    if (!child) {
      this.cleanupProcessState(new Error('knr-lsp stopped'));
      return;
    }

    try {
      if (this.isReady) {
        await this.sendRawRequest('shutdown', null, SHUTDOWN_TIMEOUT_MS).catch(() => {});
        this.sendRawNotification('exit', null);
      }
    } catch (_) {
      // Ignore shutdown errors when tearing down process
    }

    await new Promise((resolve) => {
      let settled = false;
      const done = () => {
        if (!settled) {
          settled = true;
          resolve();
        }
      };
      const timer = setTimeout(() => {
        try {
          child.kill();
        } catch (_) {
          // Ignore if already exited
        }
        done();
      }, 400);
      child.once('exit', () => {
        clearTimeout(timer);
        done();
      });
    });

    this.cleanupProcessState(new Error('knr-lsp stopped'));
  }

  async restart() {
    this.restartTimestamps = [];
    this.missingBinaryNotified = false;
    await this.stop();
    this.diagnostics.clear();
    await this.ensureStarted();
  }

  writeFrame(message) {
    if (!this.proc || !this.proc.stdin || this.proc.stdin.destroyed) {
      throw new Error('knr-lsp stdin is not writable');
    }
    this.traceLog('client -> server', message);
    const body = Buffer.from(JSON.stringify(message), 'utf8');
    const header = Buffer.from(`Content-Length: ${body.length}\r\n\r\n`, 'ascii');
    this.proc.stdin.write(Buffer.concat([header, body]));
  }

  sendRawRequest(method, params, timeoutMs) {
    const id = this.nextId++;
    const payload = { jsonrpc: '2.0', id, method };
    if (params !== undefined) {
      payload.params = params;
    }

    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        this.pending.delete(id);
        reject(new Error(`LSP request "${method}" timed out after ${timeoutMs}ms`));
      }, timeoutMs);

      this.pending.set(id, { resolve, reject, timer, method });

      try {
        this.writeFrame(payload);
      } catch (err) {
        clearTimeout(timer);
        this.pending.delete(id);
        reject(err);
      }
    });
  }

  sendRawNotification(method, params) {
    const payload = { jsonrpc: '2.0', method };
    if (params !== undefined) {
      payload.params = params;
    }
    try {
      this.writeFrame(payload);
    } catch (err) {
      this.output.appendLine(`[knr-lsp] Failed to send notification "${method}": ${err.message}`);
    }
  }

  async request(method, params, token) {
    if (token && token.isCancellationRequested) {
      return null;
    }
    try {
      await this.ensureStarted();
    } catch (_) {
      return null;
    }
    if (token && token.isCancellationRequested) {
      return null;
    }
    try {
      return await this.sendRawRequest(method, params, REQUEST_TIMEOUT_MS);
    } catch (err) {
      this.output.appendLine(`[knr-lsp] Request "${method}" failed: ${err.message}`);
      return null;
    }
  }

  notify(method, params) {
    if (!this.isReady || !this.proc) {
      this.ensureStarted().catch(() => {});
      return;
    }
    this.sendRawNotification(method, params);
  }

  syncOpenDocuments() {
    for (const doc of vscode.workspace.textDocuments) {
      if (doc.languageId === LANGUAGE_ID) {
        this.sendDidOpen(doc);
      }
    }
  }

  sendDidOpen(doc) {
    this.notify('textDocument/didOpen', {
      textDocument: {
        uri: doc.uri.toString(),
        languageId: doc.languageId,
        version: doc.version,
        text: doc.getText(),
      },
    });
  }

  sendDidChange(doc) {
    this.notify('textDocument/didChange', {
      textDocument: {
        uri: doc.uri.toString(),
        version: doc.version,
      },
      contentChanges: [
        {
          text: doc.getText(),
        },
      ],
    });
  }

  sendDidSave(doc) {
    this.notify('textDocument/didSave', {
      textDocument: {
        uri: doc.uri.toString(),
      },
      text: doc.getText(),
    });
  }

  sendDidClose(doc) {
    this.diagnostics.delete(doc.uri);
    if (this.isReady && this.proc) {
      this.sendRawNotification('textDocument/didClose', {
        textDocument: {
          uri: doc.uri.toString(),
        },
      });
    }
  }

  handleStdoutChunk(chunk) {
    this.rxBuffer = Buffer.concat([this.rxBuffer, chunk]);

    while (true) {
      let sepIndex = this.rxBuffer.indexOf('\r\n\r\n');
      let sepLen = 4;
      if (sepIndex === -1) {
        sepIndex = this.rxBuffer.indexOf('\n\n');
        sepLen = 2;
      }
      if (sepIndex === -1) {
        return;
      }

      const headerBlock = this.rxBuffer.subarray(0, sepIndex).toString('ascii');
      const match = /content-length:\s*(\d+)/i.exec(headerBlock);
      if (!match) {
        this.output.appendLine(`[knr-lsp] Dropping frame missing Content-Length header`);
        this.rxBuffer = this.rxBuffer.subarray(sepIndex + sepLen);
        continue;
      }

      const contentLength = parseInt(match[1], 10);
      if (!Number.isFinite(contentLength) || contentLength < 0 || contentLength > MAX_CONTENT_LENGTH) {
        this.output.appendLine(`[knr-lsp] Invalid Content-Length: ${match[1]}`);
        this.rxBuffer = Buffer.alloc(0);
        return;
      }

      const bodyStart = sepIndex + sepLen;
      const frameEnd = bodyStart + contentLength;
      if (this.rxBuffer.length < frameEnd) {
        return;
      }

      const bodyText = this.rxBuffer.subarray(bodyStart, frameEnd).toString('utf8');
      this.rxBuffer = this.rxBuffer.subarray(frameEnd);

      let msg;
      try {
        msg = JSON.parse(bodyText);
      } catch (err) {
        this.output.appendLine(`[knr-lsp] Invalid JSON from server: ${err.message}`);
        continue;
      }

      this.handleMessage(msg);
    }
  }

  handleMessage(msg) {
    this.traceLog('server -> client', msg);

    if (msg.id !== undefined && msg.method === undefined) {
      const entry = this.pending.get(msg.id);
      if (!entry) {
        return;
      }
      this.pending.delete(msg.id);
      clearTimeout(entry.timer);
      if (msg.error) {
        entry.reject(new Error(msg.error.message || `LSP error ${msg.error.code}`));
      } else {
        entry.resolve(msg.result);
      }
      return;
    }

    if (msg.method === 'textDocument/publishDiagnostics' && msg.params) {
      this.handlePublishDiagnostics(msg.params);
      return;
    }

    if (msg.method !== undefined && msg.id !== undefined) {
      try {
        this.writeFrame({
          jsonrpc: '2.0',
          id: msg.id,
          error: {
            code: -32601,
            message: `Client method not supported: ${msg.method}`,
          },
        });
      } catch (_) {
        // Ignore if stream closed
      }
    }
  }

  handlePublishDiagnostics(params) {
    if (!params || typeof params.uri !== 'string') {
      return;
    }
    const uri = vscode.Uri.parse(params.uri);
    const rawList = Array.isArray(params.diagnostics) ? params.diagnostics : [];
    const converted = rawList.map((d) => {
      const range = toVscodeRange(d.range);
      const severity = SEVERITY_MAP[d.severity] ?? vscode.DiagnosticSeverity.Error;
      const diag = new vscode.Diagnostic(range, String(d.message || ''), severity);
      diag.source = d.source || 'knr';
      if (d.code !== undefined && d.code !== null) {
        diag.code = d.code;
      }
      return diag;
    });
    this.diagnostics.set(uri, converted);
  }
}

let client = null;

function activate(context) {
  const output = vscode.window.createOutputChannel('knr Language Server');
  const diagnostics = vscode.languages.createDiagnosticCollection('knr');
  client = new KnrLspClient(output, diagnostics);

  const selector = { language: LANGUAGE_ID };

  const hoverProvider = vscode.languages.registerHoverProvider(selector, {
    async provideHover(document, position, token) {
      const result = await client.request(
        'textDocument/hover',
        {
          textDocument: { uri: document.uri.toString() },
          position: { line: position.line, character: position.character },
        },
        token
      );
      if (!result || token.isCancellationRequested) {
        return null;
      }
      const contents = toHoverContents(result.contents);
      if (!contents) {
        return null;
      }
      const range = result.range ? toVscodeRange(result.range) : undefined;
      return new vscode.Hover(contents, range);
    },
  });

  const completionProvider = vscode.languages.registerCompletionItemProvider(
    selector,
    {
      async provideCompletionItems(document, position, token, completionContext) {
        const ctx = {
          triggerKind: completionContext.triggerKind + 1,
        };
        if (completionContext.triggerCharacter) {
          ctx.triggerCharacter = completionContext.triggerCharacter;
        }
        const result = await client.request(
          'textDocument/completion',
          {
            textDocument: { uri: document.uri.toString() },
            position: { line: position.line, character: position.character },
            context: ctx,
          },
          token
        );
        if (!result || token.isCancellationRequested) {
          return null;
        }
        const rawItems = Array.isArray(result)
          ? result
          : Array.isArray(result.items)
            ? result.items
            : [];
        const items = rawItems.map(toCompletionItem);
        const isIncomplete = !Array.isArray(result) && Boolean(result.isIncomplete);
        return new vscode.CompletionList(items, isIncomplete);
      },
    },
    '.',
    ':',
    '"'
  );

  const symbolProvider = vscode.languages.registerDocumentSymbolProvider(selector, {
    async provideDocumentSymbols(document, token) {
      const result = await client.request(
        'textDocument/documentSymbol',
        {
          textDocument: { uri: document.uri.toString() },
        },
        token
      );
      if (!Array.isArray(result) || token.isCancellationRequested) {
        return [];
      }
      return result.map(toDocumentSymbol);
    },
  });

  const openSub = vscode.workspace.onDidOpenTextDocument((doc) => {
    if (doc.languageId === LANGUAGE_ID) {
      client.sendDidOpen(doc);
    }
  });

  const changeSub = vscode.workspace.onDidChangeTextDocument((event) => {
    if (event.document.languageId === LANGUAGE_ID) {
      client.sendDidChange(event.document);
    }
  });

  const saveSub = vscode.workspace.onDidSaveTextDocument((doc) => {
    if (doc.languageId === LANGUAGE_ID) {
      client.sendDidSave(doc);
    }
  });

  const closeSub = vscode.workspace.onDidCloseTextDocument((doc) => {
    if (doc.languageId === LANGUAGE_ID) {
      client.sendDidClose(doc);
    }
  });

  const configSub = vscode.workspace.onDidChangeConfiguration((event) => {
    if (
      event.affectsConfiguration('knr.serverPath') ||
      event.affectsConfiguration('knr.serverArgs')
    ) {
      client.restart().catch((err) => {
        output.appendLine(`[knr-lsp] Restart after config change failed: ${err.message}`);
      });
    }
  });

  const restartCmd = vscode.commands.registerCommand('knr.restartServer', async () => {
    try {
      await client.restart();
      vscode.window.showInformationMessage('knr Language Server restarted.');
    } catch (err) {
      vscode.window.showErrorMessage(`Failed to restart knr Language Server: ${err.message}`);
    }
  });

  context.subscriptions.push(
    output,
    diagnostics,
    hoverProvider,
    completionProvider,
    symbolProvider,
    openSub,
    changeSub,
    saveSub,
    closeSub,
    configSub,
    restartCmd
  );

  client.ensureStarted().catch(() => {});
}

async function deactivate() {
  if (client) {
    const active = client;
    client = null;
    await active.stop();
  }
}

module.exports = {
  activate,
  deactivate,
};
