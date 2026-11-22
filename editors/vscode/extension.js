'use strict';

const vscode = require('vscode');
const { spawn } = require('child_process');

const SUPPORTED_LANGUAGES = new Set([
  'knr',
  'knr-ssa',
]);
const SUPPORTED_EXTENSIONS = ['.tsk', '.knr', '.ssa', '.ssa.schema'];

function isSupportedDocument(doc) {
  if (!doc) {
    return false;
  }
  if (SUPPORTED_LANGUAGES.has(doc.languageId)) {
    return true;
  }
  const fsPath = doc.uri && typeof doc.uri.fsPath === 'string' ? doc.uri.fsPath.toLowerCase() : '';
  return SUPPORTED_EXTENSIONS.some((ext) => fsPath.endsWith(ext));
}
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

function toVscodeLocation(loc) {
  if (!loc || typeof loc !== 'object') {
    return null;
  }
  if (typeof loc.uri === 'string' && loc.range) {
    return new vscode.Location(vscode.Uri.parse(loc.uri), toVscodeRange(loc.range));
  }
  if (typeof loc.targetUri === 'string' && (loc.targetSelectionRange || loc.targetRange)) {
    return new vscode.Location(
      vscode.Uri.parse(loc.targetUri),
      toVscodeRange(loc.targetSelectionRange || loc.targetRange)
    );
  }
  return null;
}

function toVscodeLocations(result) {
  if (!result) {
    return [];
  }
  if (Array.isArray(result)) {
    return result.map(toVscodeLocation).filter(Boolean);
  }
  const single = toVscodeLocation(result);
  return single ? [single] : [];
}

function toVscodeTextEdit(edit) {
  return new vscode.TextEdit(toVscodeRange(edit.range), String(edit.newText ?? ''));
}

function toVscodeWorkspaceEdit(lspEdit) {
  const wsEdit = new vscode.WorkspaceEdit();
  if (!lspEdit || typeof lspEdit !== 'object') {
    return wsEdit;
  }
  if (lspEdit.changes && typeof lspEdit.changes === 'object') {
    for (const [uriStr, edits] of Object.entries(lspEdit.changes)) {
      if (!Array.isArray(edits)) {
        continue;
      }
      const uri = vscode.Uri.parse(uriStr);
      for (const edit of edits) {
        if (edit && edit.range) {
          wsEdit.replace(uri, toVscodeRange(edit.range), String(edit.newText ?? ''));
        }
      }
    }
  }
  if (Array.isArray(lspEdit.documentChanges)) {
    for (const change of lspEdit.documentChanges) {
      if (change && change.textDocument && typeof change.textDocument.uri === 'string' && Array.isArray(change.edits)) {
        const uri = vscode.Uri.parse(change.textDocument.uri);
        for (const edit of change.edits) {
          if (edit && edit.range) {
            wsEdit.replace(uri, toVscodeRange(edit.range), String(edit.newText ?? ''));
          }
        }
      }
    }
  }
  return wsEdit;
}

function toLspSeverity(severity) {
  switch (severity) {
    case vscode.DiagnosticSeverity.Error:
      return 1;
    case vscode.DiagnosticSeverity.Warning:
      return 2;
    case vscode.DiagnosticSeverity.Information:
      return 3;
    case vscode.DiagnosticSeverity.Hint:
      return 4;
    default:
      return 1;
  }
}

function toLspDiagnostic(diag) {
  const out = {
    range: {
      start: { line: diag.range.start.line, character: diag.range.start.character },
      end: { line: diag.range.end.line, character: diag.range.end.character },
    },
    severity: toLspSeverity(diag.severity),
    message: String(diag.message || ''),
  };
  if (diag.source) {
    out.source = String(diag.source);
  }
  if (diag.code !== undefined && diag.code !== null) {
    out.code = typeof diag.code === 'object' && diag.code.value !== undefined ? diag.code.value : diag.code;
  }
  return out;
}

function toVscodeCodeAction(action) {
  if (!action || typeof action !== 'object') {
    return null;
  }
  const title = typeof action.title === 'string' ? action.title : 'Quick Fix';
  const kind = vscode.CodeActionKind.QuickFix;
  const codeAction = new vscode.CodeAction(title, kind);
  if (action.isPreferred) {
    codeAction.isPreferred = true;
  }
  if (action.edit) {
    codeAction.edit = toVscodeWorkspaceEdit(action.edit);
  }
  return codeAction;
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

function toDocumentation(doc) {
  if (typeof doc === 'string') {
    return doc;
  }
  if (doc && typeof doc.value === 'string') {
    return doc.kind === 'markdown' ? new vscode.MarkdownString(doc.value) : doc.value;
  }
  return undefined;
}

function toVscodeSignatureHelp(result) {
  if (!result || !Array.isArray(result.signatures) || result.signatures.length === 0) {
    return null;
  }
  const help = new vscode.SignatureHelp();
  help.signatures = result.signatures.map((sig) => {
    const label = typeof sig.label === 'string' ? sig.label : '';
    const info = new vscode.SignatureInformation(label, toDocumentation(sig.documentation));
    if (Array.isArray(sig.parameters)) {
      info.parameters = sig.parameters.map((p) => {
        const paramLabel =
          typeof p.label === 'string' || Array.isArray(p.label) ? p.label : '';
        return new vscode.ParameterInformation(paramLabel, toDocumentation(p.documentation));
      });
    }
    return info;
  });
  help.activeSignature =
    typeof result.activeSignature === 'number' ? Math.max(0, result.activeSignature | 0) : 0;
  help.activeParameter =
    typeof result.activeParameter === 'number' ? Math.max(0, result.activeParameter | 0) : 0;
  return help;
}

function toVscodeInlayHint(hint) {
  if (!hint || !hint.position) {
    return null;
  }
  const pos = new vscode.Position(
    Math.max(0, hint.position.line | 0),
    Math.max(0, hint.position.character | 0)
  );
  const label =
    typeof hint.label === 'string'
      ? hint.label
      : Array.isArray(hint.label)
        ? hint.label.map((part) => (part && typeof part.value === 'string' ? part.value : '')).join('')
        : String(hint.label || '');
  let kind;
  if (hint.kind === 1) {
    kind = vscode.InlayHintKind.Type;
  } else if (hint.kind === 2) {
    kind = vscode.InlayHintKind.Parameter;
  }
  const item = new vscode.InlayHint(pos, label, kind);
  if (typeof hint.paddingLeft === 'boolean') {
    item.paddingLeft = hint.paddingLeft;
  }
  if (typeof hint.paddingRight === 'boolean') {
    item.paddingRight = hint.paddingRight;
  }
  if (hint.tooltip) {
    item.tooltip = toDocumentation(hint.tooltip);
  }
  return item;
}

function toVscodeSymbolInformation(sym) {
  if (!sym || typeof sym !== 'object' || !sym.location) {
    return null;
  }
  const location = toVscodeLocation(sym.location);
  if (!location) {
    return null;
  }
  const kind = SYMBOL_KIND_MAP[sym.kind] ?? vscode.SymbolKind.Function;
  return new vscode.SymbolInformation(
    sym.name || '<anonymous>',
    kind,
    typeof sym.containerName === 'string' ? sym.containerName : '',
    location
  );
}

function toVscodeFoldingRange(fr) {
  if (!fr || typeof fr.startLine !== 'number' || typeof fr.endLine !== 'number') {
    return null;
  }
  let kind;
  if (fr.kind === 'comment') {
    kind = vscode.FoldingRangeKind.Comment;
  } else if (fr.kind === 'imports') {
    kind = vscode.FoldingRangeKind.Imports;
  } else if (fr.kind === 'region') {
    kind = vscode.FoldingRangeKind.Region;
  }
  return new vscode.FoldingRange(fr.startLine | 0, fr.endLine | 0, kind);
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
        version: '0.4.0',
      },
      rootUri: folders.length > 0 ? folders[0].uri.toString() : null,
      workspaceFolders: folders.map((f) => ({
        uri: f.uri.toString(),
        name: f.name,
      })),
      capabilities: {
        workspace: {
          symbol: {
            dynamicRegistration: false,
          },
        },
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
          signatureHelp: {
            dynamicRegistration: false,
            signatureInformation: {
              documentationFormat: ['markdown', 'plaintext'],
              parameterInformation: {
                labelOffsetSupport: false,
              },
            },
          },
          inlayHint: {
            dynamicRegistration: false,
          },
          foldingRange: {
            dynamicRegistration: false,
            lineFoldingOnly: true,
          },
          documentSymbol: {
            hierarchicalDocumentSymbolSupport: true,
          },
          definition: {
            dynamicRegistration: false,
          },
          references: {
            dynamicRegistration: false,
          },
          rename: {
            dynamicRegistration: false,
            prepareSupport: true,
          },
          codeAction: {
            dynamicRegistration: false,
            codeActionLiteralSupport: {
              codeActionKind: {
                valueSet: ['quickfix'],
              },
            },
          },
          formatting: {
            dynamicRegistration: false,
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
        const hasOpenKnr = vscode.workspace.textDocuments.some((doc) =>
          isSupportedDocument(doc)
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
      if (isSupportedDocument(doc)) {
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

  const selector = [
    { language: 'knr' },
    { language: 'knr-ssa' },
  ];

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
    '"',
    '/',
    '{',
    '<',
    '@'
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

  const definitionProvider = vscode.languages.registerDefinitionProvider(selector, {
    async provideDefinition(document, position, token) {
      const result = await client.request(
        'textDocument/definition',
        {
          textDocument: { uri: document.uri.toString() },
          position: { line: position.line, character: position.character },
        },
        token
      );
      if (!result || token.isCancellationRequested) {
        return null;
      }
      if (Array.isArray(result)) {
        const locs = toVscodeLocations(result);
        return locs.length > 0 ? locs : null;
      }
      return toVscodeLocation(result);
    },
  });

  const referenceProvider = vscode.languages.registerReferenceProvider(selector, {
    async provideReferences(document, position, refContext, token) {
      const result = await client.request(
        'textDocument/references',
        {
          textDocument: { uri: document.uri.toString() },
          position: { line: position.line, character: position.character },
          context: {
            includeDeclaration: refContext ? Boolean(refContext.includeDeclaration) : true,
          },
        },
        token
      );
      if (!result || token.isCancellationRequested) {
        return [];
      }
      return toVscodeLocations(result);
    },
  });

  const renameProvider = vscode.languages.registerRenameProvider(selector, {
    async prepareRename(document, position, token) {
      const result = await client.request(
        'textDocument/prepareRename',
        {
          textDocument: { uri: document.uri.toString() },
          position: { line: position.line, character: position.character },
        },
        token
      );
      if (token.isCancellationRequested) {
        return null;
      }
      if (!result || !result.range) {
        throw new Error('Cannot rename this symbol.');
      }
      return {
        range: toVscodeRange(result.range),
        placeholder: typeof result.placeholder === 'string' ? result.placeholder : '',
      };
    },

    async provideRenameEdits(document, position, newName, token) {
      const result = await client.request(
        'textDocument/rename',
        {
          textDocument: { uri: document.uri.toString() },
          position: { line: position.line, character: position.character },
          newName,
        },
        token
      );
      if (!result || token.isCancellationRequested) {
        return null;
      }
      return toVscodeWorkspaceEdit(result);
    },
  });

  const codeActionsProvider = vscode.languages.registerCodeActionsProvider(
    selector,
    {
      async provideCodeActions(document, range, actionContext, token) {
        const ctx = {
          diagnostics: Array.isArray(actionContext && actionContext.diagnostics)
            ? actionContext.diagnostics.map(toLspDiagnostic)
            : [],
        };
        if (actionContext && actionContext.only && typeof actionContext.only.value === 'string') {
          ctx.only = [actionContext.only.value];
        }
        const result = await client.request(
          'textDocument/codeAction',
          {
            textDocument: { uri: document.uri.toString() },
            range: {
              start: { line: range.start.line, character: range.start.character },
              end: { line: range.end.line, character: range.end.character },
            },
            context: ctx,
          },
          token
        );
        if (!Array.isArray(result) || token.isCancellationRequested) {
          return [];
        }
        return result.map(toVscodeCodeAction).filter(Boolean);
      },
    },
    {
      providedCodeActionKinds: [vscode.CodeActionKind.QuickFix],
    }
  );

  const formattingProvider = vscode.languages.registerDocumentFormattingEditProvider(selector, {
    async provideDocumentFormattingEdits(document, options, token) {
      const result = await client.request(
        'textDocument/formatting',
        {
          textDocument: { uri: document.uri.toString() },
          options: {
            tabSize: options && typeof options.tabSize === 'number' ? options.tabSize : 4,
            insertSpaces: options && typeof options.insertSpaces === 'boolean' ? options.insertSpaces : true,
          },
        },
        token
      );
      if (!Array.isArray(result) || token.isCancellationRequested) {
        return [];
      }
      return result.filter((e) => e && e.range).map(toVscodeTextEdit);
    },
  });

  const signatureHelpProvider = vscode.languages.registerSignatureHelpProvider(
    selector,
    {
      async provideSignatureHelp(document, position, token, sigContext) {
        const params = {
          textDocument: { uri: document.uri.toString() },
          position: { line: position.line, character: position.character },
        };
        if (sigContext) {
          params.context = {
            triggerKind: sigContext.triggerKind,
            isRetrigger: Boolean(sigContext.isRetrigger),
          };
          if (sigContext.triggerCharacter) {
            params.context.triggerCharacter = sigContext.triggerCharacter;
          }
        }
        const result = await client.request('textDocument/signatureHelp', params, token);
        if (!result || token.isCancellationRequested) {
          return null;
        }
        return toVscodeSignatureHelp(result);
      },
    },
    '(',
    ','
  );

  const inlayHintsProvider = vscode.languages.registerInlayHintsProvider(selector, {
    async provideInlayHints(document, range, token) {
      const result = await client.request(
        'textDocument/inlayHint',
        {
          textDocument: { uri: document.uri.toString() },
          range: {
            start: { line: range.start.line, character: range.start.character },
            end: { line: range.end.line, character: range.end.character },
          },
        },
        token
      );
      if (!Array.isArray(result) || token.isCancellationRequested) {
        return [];
      }
      return result.map(toVscodeInlayHint).filter(Boolean);
    },
  });

  const workspaceSymbolProvider = vscode.languages.registerWorkspaceSymbolProvider({
    async provideWorkspaceSymbols(query, token) {
      const result = await client.request(
        'workspace/symbol',
        {
          query: typeof query === 'string' ? query : '',
        },
        token
      );
      if (!Array.isArray(result) || token.isCancellationRequested) {
        return [];
      }
      return result.map(toVscodeSymbolInformation).filter(Boolean);
    },
  });

  const foldingRangeProvider = vscode.languages.registerFoldingRangeProvider(selector, {
    async provideFoldingRanges(document, _foldContext, token) {
      const result = await client.request(
        'textDocument/foldingRange',
        {
          textDocument: { uri: document.uri.toString() },
        },
        token
      );
      if (!Array.isArray(result) || token.isCancellationRequested) {
        return [];
      }
      return result.map(toVscodeFoldingRange).filter(Boolean);
    },
  });

  const openSub = vscode.workspace.onDidOpenTextDocument((doc) => {
    if (isSupportedDocument(doc)) {
      client.sendDidOpen(doc);
    }
  });

  const changeSub = vscode.workspace.onDidChangeTextDocument((event) => {
    if (isSupportedDocument(event.document)) {
      client.sendDidChange(event.document);
    }
  });

  const saveSub = vscode.workspace.onDidSaveTextDocument((doc) => {
    if (isSupportedDocument(doc)) {
      client.sendDidSave(doc);
    }
  });

  const closeSub = vscode.workspace.onDidCloseTextDocument((doc) => {
    if (isSupportedDocument(doc)) {
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
    definitionProvider,
    referenceProvider,
    renameProvider,
    codeActionsProvider,
    formattingProvider,
    signatureHelpProvider,
    inlayHintsProvider,
    workspaceSymbolProvider,
    foldingRangeProvider,
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
