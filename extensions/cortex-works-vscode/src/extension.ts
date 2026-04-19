import { ChildProcessWithoutNullStreams, spawn } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';
import readline from 'node:readline';
import * as vscode from 'vscode';

type PendingRequest = {
  resolve: (value: any) => void;
  reject: (reason?: unknown) => void;
};

type ToolDefinition = {
  name: string;
  displayName: string;
};

type BridgeToolSchema = {
  name: string;
  description?: string;
  inputSchema?: unknown;
};

type BridgeResponse = {
  ok?: boolean;
  result?: Record<string, unknown>;
  error?: string;
};

type McpResponse = {
  result?: {
    content?: Array<{ type?: string; text?: string }>;
    isError?: boolean;
    tools?: BridgeToolSchema[];
  };
  error?: {
    message?: string;
  };
};

type ToolRunResult = {
  ok: boolean;
  output: string;
  ms: number;
};

type CatalogResult = {
  ok: boolean;
  names: string[];
  ms: number;
  error?: string;
};

type SelfTestCase = {
  id: string;
  toolName: string;
  args: Record<string, unknown>;
  reset?: () => void;
};

const TOOL_DEFINITIONS: ToolDefinition[] = [
  { name: 'cortex_code_explorer', displayName: 'Cortex Code Explorer' },
  { name: 'cortex_symbol_analyzer', displayName: 'Cortex Symbol Analyzer' },
  { name: 'cortex_chronos', displayName: 'Cortex Chronos' },
  { name: 'cortex_manage_ast_languages', displayName: 'Cortex Manage AST Languages' },
  { name: 'cortex_act_edit_ast', displayName: 'Cortex Act Edit AST' },
  { name: 'cortex_act_edit_data_graph', displayName: 'Cortex Act Edit Data Graph' },
  { name: 'cortex_act_edit_markup', displayName: 'Cortex Act Edit Markup' },
  { name: 'cortex_act_sql_surgery', displayName: 'Cortex Act SQL Surgery' },
  { name: 'cortex_act_shell_exec', displayName: 'Cortex Act Shell Exec' },
  { name: 'cortex_act_batch_execute', displayName: 'Cortex Act Batch Execute' },
  { name: 'cortex_search_exact', displayName: 'Cortex Search Exact' },
  { name: 'cortex_mcp_hot_reload', displayName: 'Cortex MCP Hot Reload' },
  { name: 'cortex_fs_manage', displayName: 'Cortex FS Manage' },
];

const EXPECTED_TOOL_NAMES = TOOL_DEFINITIONS.map((tool) => tool.name);
const REPORT_RELATIVE_PATH = path.join('target', 'cortex-works-vscode', 'extension-self-test.json');
const FIXTURE_RELATIVE_DIR = path.join('target', 'cortex-works-vscode', 'fixtures');
const HOT_RELOAD_SETTLE_MS = 900;
const DEFAULT_RPC_TIMEOUT_MS = 30_000;
const CATALOG_TIMEOUT_MS = 10_000;
const TOOL_CALL_TIMEOUT_MS = 60_000;

export function activate(context: vscode.ExtensionContext): void {
  const output = vscode.window.createOutputChannel('Cortex Works', { log: true });
  const runtime = new BridgeRuntime(context, output);

  context.subscriptions.push(output, runtime);
  context.subscriptions.push(registerCommands(runtime));
  context.subscriptions.push(registerLanguageModelTools(runtime));
}

export function deactivate(): void {
  // Disposables handle shutdown.
}

function registerCommands(runtime: BridgeRuntime): vscode.Disposable {
  return vscode.Disposable.from(
    vscode.commands.registerCommand('cortexWorks.showStatus', async () => {
      const summary = await runtime.statusText();
      await vscode.window.showInformationMessage(summary, { modal: true }, 'OK');
    }),
    vscode.commands.registerCommand('cortexWorks.restartNativeBridge', async () => {
      runtime.restartBridge();
      await vscode.window.showInformationMessage('Cortex Works native bridge restarted.');
    }),
    vscode.commands.registerCommand('cortexWorks.runParitySelfTest', async () => {
      await runParitySelfTest(runtime);
    }),
  );
}

function registerLanguageModelTools(runtime: BridgeRuntime): vscode.Disposable {
  return vscode.Disposable.from(
    ...TOOL_DEFINITIONS.map((tool) =>
      vscode.lm.registerTool<Record<string, unknown>>(tool.name, {
        prepareInvocation: async () => ({
          invocationMessage: `Running ${tool.name} through the native Cortex bridge`,
          confirmationMessages: {
            title: `Run ${tool.displayName}`,
            message: new vscode.MarkdownString(`Invoke ${tool.name} through the extension-native Cortex bridge?`),
          },
        }),
        invoke: async (options) => {
          const output = await runtime.invokeTool(tool.name, asObject(options.input));
          return asToolResult(output);
        },
      }),
    ),
  );
}

function asToolResult(text: string): vscode.LanguageModelToolResult {
  return new vscode.LanguageModelToolResult([new vscode.LanguageModelTextPart(text)]);
}

function asObject(value: unknown): Record<string, unknown> {
  if (!value || typeof value !== 'object' || Array.isArray(value)) {
    return {};
  }
  return value as Record<string, unknown>;
}

function requireWorkspaceRoot(): vscode.WorkspaceFolder {
  const folder = vscode.workspace.workspaceFolders?.[0];
  if (!folder || folder.uri.scheme !== 'file') {
    throw new Error('A file-backed workspace folder is required for Cortex Works.');
  }
  return folder;
}

function executableName(baseName: string): string {
  return process.platform === 'win32' ? `${baseName}.exe` : baseName;
}

function normalizeText(text: string): string {
  return text.replace(/\r\n/g, '\n').trim();
}

function trimForReport(text: string, maxChars = 8000): string {
  return text.length <= maxChars ? text : `${text.slice(0, maxChars)}\n...<truncated>`;
}

function wait(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

function errorText(error: unknown): string {
  if (error instanceof Error) {
    return error.message;
  }
  return String(error);
}

function ensureDirectory(dirPath: string): void {
  fs.mkdirSync(dirPath, { recursive: true });
}

function writeFile(filePath: string, content: string): void {
  ensureDirectory(path.dirname(filePath));
  fs.writeFileSync(filePath, content, 'utf8');
}

class BridgeRuntime implements vscode.Disposable {
  private bridgeClient: ExtensionBridgeClient | undefined;
  private baselineClient: McpProtocolClient | undefined;

  constructor(
    private readonly context: vscode.ExtensionContext,
    private readonly output: vscode.LogOutputChannel,
  ) {}

  async invokeTool(name: string, args: Record<string, unknown>): Promise<string> {
    const client = await this.getBridgeClient();
    const result = await client.callTool(name, args);
    if (name === 'cortex_mcp_hot_reload') {
      client.markUninitialized();
      void wait(HOT_RELOAD_SETTLE_MS).then(() => this.restartBridge());
    }
    return result;
  }

  async callBaselineTool(name: string, args: Record<string, unknown>): Promise<string> {
    const client = await this.getBaselineClient();
    const result = await client.callTool(name, args);
    if (name === 'cortex_mcp_hot_reload') {
      client.markUninitialized();
    }
    return result;
  }

  async listBridgeTools(): Promise<string[]> {
    const tools = await (await this.getBridgeClient()).listTools();
    return tools.map((tool) => tool.name);
  }

  async listBaselineTools(): Promise<string[]> {
    const tools = await (await this.getBaselineClient()).listTools();
    return tools.map((tool) => tool.name);
  }

  async statusText(): Promise<string> {
    const names = await this.listBridgeTools();
    return [
      `Bridge binary: ${this.resolveBinaryPath('cortex-extension-bridge')}`,
      `Baseline MCP binary: ${this.resolveBinaryPath('cortex-mcp')}`,
      `Tool count: ${names.length}`,
      `Tools: ${names.join(', ')}`,
      `Latest self-test report: ${this.reportFilePath()}`,
    ].join('\n');
  }

  reportFilePath(): string {
    return path.join(requireWorkspaceRoot().uri.fsPath, REPORT_RELATIVE_PATH);
  }

  restartBridge(): void {
    this.bridgeClient?.dispose();
    this.bridgeClient = undefined;
  }

  restartBaseline(): void {
    this.baselineClient?.dispose();
    this.baselineClient = undefined;
  }

  resolveBinaryPath(binaryName: 'cortex-extension-bridge' | 'cortex-mcp'): string {
    const configuration = vscode.workspace.getConfiguration('cortexWorks');
    const overrideKey = binaryName === 'cortex-extension-bridge'
      ? 'binaryOverrides.extensionBridge'
      : 'binaryOverrides.cortexMcp';
    const override = configuration.get<string>(overrideKey)?.trim();
    const candidate = override || this.context.asAbsolutePath(
      path.join('resources', 'sidecars', `${process.platform}-${process.arch}`, executableName(binaryName)),
    );

    if (!fs.existsSync(candidate)) {
      throw new Error(
        `Missing binary '${binaryName}' at ${candidate}. Run 'npm run stage:sidecars' or configure cortexWorks.${overrideKey}.`,
      );
    }

    return candidate;
  }

  baseEnvironment(): NodeJS.ProcessEnv {
    const env: NodeJS.ProcessEnv = { ...process.env, CORTEX_MODE: 'extension-native' };
    const workspace = vscode.workspace.workspaceFolders?.[0];
    if (workspace?.uri.scheme === 'file') {
      env.VSCODE_WORKSPACE_FOLDER = workspace.uri.fsPath;
      env.VSCODE_CWD = workspace.uri.fsPath;
      env.CORTEXAST_ROOT = workspace.uri.fsPath;
    }
    return env;
  }

  dispose(): void {
    this.restartBridge();
    this.restartBaseline();
  }

  private async getBridgeClient(): Promise<ExtensionBridgeClient> {
    if (!this.bridgeClient) {
      this.bridgeClient = new ExtensionBridgeClient(
        new JsonLineProcess({
          label: 'cortex-extension-bridge',
          command: this.resolveBinaryPath('cortex-extension-bridge'),
          args: [],
          env: this.baseEnvironment(),
          cwd: requireWorkspaceRoot().uri.fsPath,
        }, this.output),
      );
    }
    return this.bridgeClient;
  }

  private async getBaselineClient(): Promise<McpProtocolClient> {
    if (!this.baselineClient) {
      this.baselineClient = new McpProtocolClient(
        new JsonLineProcess({
          label: 'cortex-mcp',
          command: this.resolveBinaryPath('cortex-mcp'),
          args: [],
          env: this.baseEnvironment(),
          cwd: requireWorkspaceRoot().uri.fsPath,
        }, this.output),
        this.context.extension.packageJSON.version,
      );
    }
    return this.baselineClient;
  }
}

class ExtensionBridgeClient implements vscode.Disposable {
  private initialized = false;

  constructor(private readonly process: JsonLineProcess) {}

  async listTools(): Promise<BridgeToolSchema[]> {
    await this.ensureInitialized();
    const response = await this.process.request<BridgeResponse>('list_tools', {}, CATALOG_TIMEOUT_MS);
    if (!response.ok) {
      throw new Error(response.error ?? 'Failed to list tools from the extension bridge.');
    }

    const tools = response.result?.tools;
    if (!Array.isArray(tools)) {
      throw new Error('Extension bridge returned an invalid tool catalog.');
    }
    return tools as BridgeToolSchema[];
  }

  async callTool(name: string, args: Record<string, unknown>): Promise<string> {
    await this.ensureInitialized();
    const response = await this.process.request<BridgeResponse>(
      'call_tool',
      { name, arguments: args },
      TOOL_CALL_TIMEOUT_MS,
    );
    if (!response.ok) {
      throw new Error(response.error ?? `Tool '${name}' failed in the extension bridge.`);
    }

    const text = response.result?.text;
    if (typeof text !== 'string') {
      throw new Error(`Tool '${name}' returned an invalid bridge payload.`);
    }
    return text;
  }

  markUninitialized(): void {
    this.initialized = false;
  }

  dispose(): void {
    this.process.dispose();
    this.initialized = false;
  }

  private async ensureInitialized(): Promise<void> {
    if (this.initialized) {
      return;
    }

    const workspaceFolders = (vscode.workspace.workspaceFolders ?? []).map((folder) => ({
      name: folder.name,
      uri: folder.uri.toString(),
    }));
    const response = await this.process.request<BridgeResponse>(
      'initialize',
      { workspaceFolders },
      CATALOG_TIMEOUT_MS,
    );
    if (!response.ok) {
      throw new Error(response.error ?? 'Failed to initialize the extension bridge.');
    }

    this.initialized = true;
  }
}

class McpProtocolClient implements vscode.Disposable {
  private initialized = false;

  constructor(
    private readonly process: JsonLineProcess,
    private readonly version: string,
  ) {}

  async listTools(): Promise<BridgeToolSchema[]> {
    await this.ensureInitialized();
    const response = await this.process.request<McpResponse>('tools/list', {}, CATALOG_TIMEOUT_MS);
    if (response.error?.message) {
      throw new Error(response.error.message);
    }
    const tools = response.result?.tools;
    if (!Array.isArray(tools)) {
      throw new Error('cortex-mcp returned an invalid tool catalog.');
    }
    return tools;
  }

  async callTool(name: string, args: Record<string, unknown>): Promise<string> {
    await this.ensureInitialized();
    const response = await this.process.request<McpResponse>(
      'tools/call',
      { name, arguments: args },
      TOOL_CALL_TIMEOUT_MS,
    );
    const text = extractMcpText(response);
    if (response.error?.message) {
      throw new Error(response.error.message);
    }
    if (response.result?.isError) {
      throw new Error(text || `Tool '${name}' failed through cortex-mcp.`);
    }
    return text;
  }

  markUninitialized(): void {
    this.initialized = false;
  }

  dispose(): void {
    this.process.dispose();
    this.initialized = false;
  }

  private async ensureInitialized(): Promise<void> {
    if (this.initialized) {
      return;
    }

    const workspaceFolders = (vscode.workspace.workspaceFolders ?? []).map((folder) => ({
      name: folder.name,
      uri: folder.uri.toString(),
    }));
    const response = await this.process.request<McpResponse>(
      'initialize',
      {
        protocolVersion: '2024-11-05',
        clientInfo: { name: 'cortex-works-vscode', version: this.version },
        capabilities: {},
        workspaceFolders,
      },
      CATALOG_TIMEOUT_MS,
    );

    if (response.error?.message) {
      throw new Error(response.error.message);
    }
    this.initialized = true;
  }
}

class JsonLineProcess implements vscode.Disposable {
  private child: ChildProcessWithoutNullStreams | undefined;
  private nextId = 1;
  private readonly pending = new Map<number, PendingRequest>();
  private starting: Promise<void> | undefined;

  constructor(
    private readonly spec: {
      label: string;
      command: string;
      args: string[];
      env: NodeJS.ProcessEnv;
      cwd?: string;
    },
    private readonly output: vscode.LogOutputChannel,
  ) {}

  async request<T>(method: string, params: unknown, timeoutMs = DEFAULT_RPC_TIMEOUT_MS): Promise<T> {
    await this.start();
    const child = this.child;
    if (!child || !child.stdin.writable) {
      throw new Error(`${this.spec.label} is not writable.`);
    }

    const id = this.nextId++;
    const payload = JSON.stringify({ id, method, params }) + '\n';

    return new Promise<T>((resolve, reject) => {
      const timeoutHandle = setTimeout(() => {
        if (!this.pending.delete(id)) {
          return;
        }

        this.output.warn(
          `${this.spec.label} request '${method}' timed out after ${timeoutMs} ms; restarting process`,
        );
        if (this.child && !this.child.killed) {
          this.child.kill();
        }
        reject(new Error(`${this.spec.label} request '${method}' timed out after ${timeoutMs} ms.`));
      }, timeoutMs);

      this.pending.set(id, {
        resolve: (value) => {
          clearTimeout(timeoutHandle);
          resolve(value as T);
        },
        reject: (reason) => {
          clearTimeout(timeoutHandle);
          reject(reason);
        },
      });

      child.stdin.write(payload, (error) => {
        if (error) {
          clearTimeout(timeoutHandle);
          this.pending.delete(id);
          reject(error);
        }
      });
    });
  }

  dispose(): void {
    this.rejectPending(new Error(`${this.spec.label} was disposed.`));
    if (this.child && !this.child.killed) {
      this.child.kill();
    }
    this.child = undefined;
  }

  private async start(): Promise<void> {
    if (this.child) {
      return;
    }
    if (this.starting) {
      return this.starting;
    }

    this.starting = new Promise<void>((resolve, reject) => {
      const child = spawn(this.spec.command, this.spec.args, {
        cwd: this.spec.cwd,
        env: this.spec.env,
        stdio: 'pipe',
      });

      child.once('spawn', () => {
        this.child = child;
        this.output.info(`started ${this.spec.label}: ${this.spec.command}`);
        this.attachLineReaders(child);
        resolve();
      });

      child.once('error', (error) => {
        reject(error);
      });

      child.once('exit', (code, signal) => {
        this.output.warn(`${this.spec.label} exited with code=${code ?? 'null'} signal=${signal ?? 'null'}`);
        this.child = undefined;
        this.rejectPending(new Error(`${this.spec.label} exited unexpectedly.`));
      });
    }).finally(() => {
      this.starting = undefined;
    });

    return this.starting;
  }

  private attachLineReaders(child: ChildProcessWithoutNullStreams): void {
    readline.createInterface({ input: child.stdout }).on('line', (line) => {
      if (!line.trim()) {
        return;
      }

      let payload: unknown;
      try {
        payload = JSON.parse(line);
      } catch {
        this.output.error(`Invalid JSON from ${this.spec.label}: ${line}`);
        return;
      }

      const envelope = payload as { id?: unknown };
      if (typeof envelope.id !== 'number') {
        this.output.debug(`${this.spec.label} notification: ${line}`);
        return;
      }

      const pending = this.pending.get(envelope.id);
      if (!pending) {
        return;
      }

      this.pending.delete(envelope.id);
      pending.resolve(payload);
    });

    readline.createInterface({ input: child.stderr }).on('line', (line) => {
      this.output.debug(`${this.spec.label} stderr: ${line}`);
    });
  }

  private rejectPending(error: Error): void {
    for (const [id, pending] of this.pending.entries()) {
      this.pending.delete(id);
      pending.reject(error);
    }
  }
}

function extractMcpText(response: McpResponse): string {
  return response.result?.content
    ?.filter((entry) => entry.type === 'text' && typeof entry.text === 'string')
    .map((entry) => entry.text as string)
    .join('\n') ?? '';
}

async function runParitySelfTest(runtime: BridgeRuntime): Promise<void> {
  const workspaceRoot = requireWorkspaceRoot().uri.fsPath;
  const reportPath = runtime.reportFilePath();
  const fixtureRoot = path.join(workspaceRoot, FIXTURE_RELATIVE_DIR);

  fs.rmSync(fixtureRoot, { recursive: true, force: true });
  ensureDirectory(fixtureRoot);

  const cases = createSelfTestCases(workspaceRoot, fixtureRoot);
  const caseReports: Array<Record<string, unknown>> = [];
  let bridgeCatalog: CatalogResult = { ok: false, names: [], ms: 0, error: 'not started' };
  let baselineCatalog: CatalogResult = { ok: false, names: [], ms: 0, error: 'not started' };

  const persistReport = (payload: Record<string, unknown>): void => {
    writeFile(reportPath, `${JSON.stringify(payload, null, 2)}\n`);
  };

  const buildReport = (status: 'running' | 'completed' | 'failed', fatalError?: string) => {
    const catalogMatch = bridgeCatalog.ok
      && baselineCatalog.ok
      && arraysEqual(bridgeCatalog.names, EXPECTED_TOOL_NAMES)
      && arraysEqual(baselineCatalog.names, EXPECTED_TOOL_NAMES)
      && arraysEqual(bridgeCatalog.names, baselineCatalog.names);

    const mismatchedCases = caseReports.filter((report) => report.match === false).length;
    const failedCases = caseReports.filter((report) => {
      const extension = report.extension as { ok: boolean };
      const mcp = report.mcp as { ok: boolean };
      return !extension.ok || !mcp.ok;
    }).length;

    return {
      generatedAt: new Date().toISOString(),
      status,
      fatalError,
      workspaceRoot,
      reportPath,
      expectedTools: EXPECTED_TOOL_NAMES,
      catalog: {
        extension: bridgeCatalog,
        mcp: baselineCatalog,
        match: catalogMatch,
      },
      summary: {
        totalCases: cases.length,
        completedCases: caseReports.length,
        mismatchedCases,
        failedCases,
        allPassed: status === 'completed' && catalogMatch && mismatchedCases === 0 && failedCases === 0,
      },
      cases: caseReports,
    };
  };

  persistReport(buildReport('running'));

  try {
    bridgeCatalog = await measureCatalog(() => runtime.listBridgeTools());
    baselineCatalog = await measureCatalog(() => runtime.listBaselineTools());
    persistReport(buildReport('running'));

    for (const testCase of cases) {
      testCase.reset?.();
      const nativeResult = await measureToolRun(() => runtime.invokeTool(testCase.toolName, testCase.args));
      testCase.reset?.();
      const mcpResult = await measureToolRun(() => runtime.callBaselineTool(testCase.toolName, testCase.args));

      const outputsMatch = nativeResult.ok === mcpResult.ok
        && normalizeText(nativeResult.output) === normalizeText(mcpResult.output);

      const report: Record<string, unknown> = {
        id: testCase.id,
        toolName: testCase.toolName,
        args: testCase.args,
        extension: {
          ok: nativeResult.ok,
          ms: nativeResult.ms,
          output: trimForReport(nativeResult.output),
        },
        mcp: {
          ok: mcpResult.ok,
          ms: mcpResult.ms,
          output: trimForReport(mcpResult.output),
        },
        match: outputsMatch,
      };

      if (testCase.toolName === 'cortex_mcp_hot_reload') {
        await wait(HOT_RELOAD_SETTLE_MS);
        const bridgeRecovery = await measureCatalog(() => runtime.listBridgeTools());
        const baselineRecovery = await measureCatalog(() => runtime.listBaselineTools());
        report.recovery = {
          extension: bridgeRecovery,
          mcp: baselineRecovery,
          match: bridgeRecovery.ok && baselineRecovery.ok,
        };
      }

      caseReports.push(report);
      persistReport(buildReport('running'));
    }

    const finalReport = buildReport('completed');
    persistReport(finalReport);

    const message = finalReport.summary.allPassed
      ? `Cortex Works self-test passed. Report: ${reportPath}`
      : `Cortex Works self-test found issues. Report: ${reportPath}`;

    if (finalReport.summary.allPassed) {
      void vscode.window.showInformationMessage(message);
    } else {
      void vscode.window.showWarningMessage(message);
    }
  } catch (error) {
    const finalReport = buildReport('failed', errorText(error));
    persistReport(finalReport);
    void vscode.window.showErrorMessage(
      `Cortex Works self-test failed early. Report: ${reportPath}`,
    );
  }
}

function arraysEqual(left: string[], right: string[]): boolean {
  return left.length === right.length && left.every((value, index) => value === right[index]);
}

async function measureToolRun(run: () => Promise<string>): Promise<ToolRunResult> {
  const startedAt = Date.now();
  try {
    const output = await run();
    return { ok: true, output, ms: Date.now() - startedAt };
  } catch (error) {
    return { ok: false, output: errorText(error), ms: Date.now() - startedAt };
  }
}

async function measureCatalog(run: () => Promise<string[]>): Promise<CatalogResult> {
  const startedAt = Date.now();
  try {
    const names = await run();
    return { ok: true, names, ms: Date.now() - startedAt };
  } catch (error) {
    return { ok: false, names: [], ms: Date.now() - startedAt, error: errorText(error) };
  }
}

function createSelfTestCases(repoRoot: string, fixtureRoot: string): SelfTestCase[] {
  const searchToken = 'CORTEX_EXTENSION_SEARCH_TOKEN';
  const searchFile = path.join(fixtureRoot, 'search_fixture.rs');
  const searchContent = `fn marker() {\n    println!(\"${searchToken}\");\n}\n`;

  const jsonFile = path.join(fixtureRoot, 'config.json');
  const jsonContent = `${JSON.stringify({ feature: { enabled: false } }, null, 2)}\n`;

  const markupFile = path.join(fixtureRoot, 'doc.md');
  const markupContent = '# Title\n\n## Setup\nOriginal setup text.\n';

  const sqlFile = path.join(fixtureRoot, 'schema.sql');
  const sqlContent = 'CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT NOT NULL);\n';

  const rustFile = path.join(fixtureRoot, 'edit_fixture.rs');
  const rustContent = 'fn greet() { println!("hello"); }\n';

  const fsManageFile = path.join(fixtureRoot, 'fs-manage-output.txt');

  const writeSearchFixture = (): void => writeFile(searchFile, searchContent);
  const writeJsonFixture = (): void => writeFile(jsonFile, jsonContent);
  const writeMarkupFixture = (): void => writeFile(markupFile, markupContent);
  const writeSqlFixture = (): void => writeFile(sqlFile, sqlContent);
  const writeRustFixture = (): void => writeFile(rustFile, rustContent);

  writeSearchFixture();
  writeJsonFixture();
  writeMarkupFixture();
  writeSqlFixture();
  writeRustFixture();

  return [
    {
      id: 'cortex_code_explorer.workspace_topology',
      toolName: 'cortex_code_explorer',
      args: { action: 'workspace_topology', repoPath: repoRoot },
    },
    {
      id: 'cortex_code_explorer.deep_slice',
      toolName: 'cortex_code_explorer',
      args: {
        action: 'deep_slice',
        target: 'crates/cortex-mcp/src/main.rs',
        single_file: true,
        skeleton_only: true,
        budget_tokens: 4000,
        max_chars: 4000,
      },
    },
    {
      id: 'cortex_symbol_analyzer.read_source',
      toolName: 'cortex_symbol_analyzer',
      args: {
        action: 'read_source',
        path: 'crates/cortex-mcp/src/tools/mod.rs',
        symbol_name: 'dispatch',
        skeleton_only: true,
      },
    },
    {
      id: 'cortex_chronos.list_checkpoints',
      toolName: 'cortex_chronos',
      args: { action: 'list_checkpoints', repoPath: repoRoot },
    },
    {
      id: 'cortex_manage_ast_languages.status',
      toolName: 'cortex_manage_ast_languages',
      args: { action: 'status', repoPath: repoRoot },
    },
    {
      id: 'cortex_search_exact.basic',
      toolName: 'cortex_search_exact',
      args: {
        regex_pattern: searchToken,
        project_path: fixtureRoot,
        file_extension: 'rs',
      },
      reset: writeSearchFixture,
    },
    {
      id: 'cortex_act_shell_exec.pwd',
      toolName: 'cortex_act_shell_exec',
      args: { command: 'pwd', cwd: repoRoot },
    },
    {
      id: 'cortex_fs_manage.write',
      toolName: 'cortex_fs_manage',
      args: {
        action: 'write',
        paths: [fsManageFile],
        content: 'hello from cortex fs manage\n',
      },
      reset: () => fs.rmSync(fsManageFile, { force: true }),
    },
    {
      id: 'cortex_act_edit_data_graph.json_set',
      toolName: 'cortex_act_edit_data_graph',
      args: {
        file: jsonFile,
        edits: [{ target: '$.feature.enabled', action: 'set', value: 'true' }],
      },
      reset: writeJsonFixture,
    },
    {
      id: 'cortex_act_edit_markup.replace_heading',
      toolName: 'cortex_act_edit_markup',
      args: {
        file: markupFile,
        edits: [{ target: 'heading:Setup', action: 'replace', code: '## Setup\nUpdated setup text.\n' }],
      },
      reset: writeMarkupFixture,
    },
    {
      id: 'cortex_act_sql_surgery.replace_table',
      toolName: 'cortex_act_sql_surgery',
      args: {
        file: sqlFile,
        edits: [{
          target: 'create_table:users',
          action: 'replace',
          code: 'CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT NOT NULL, email TEXT NOT NULL);',
        }],
      },
      reset: writeSqlFixture,
    },
    {
      id: 'cortex_act_edit_ast.replace_function',
      toolName: 'cortex_act_edit_ast',
      args: {
        file: rustFile,
        edits: [{ target: 'greet', action: 'replace', code: 'fn greet() { println!("updated"); }' }],
      },
      reset: writeRustFixture,
    },
    {
      id: 'cortex_act_batch_execute.search_plus_shell',
      toolName: 'cortex_act_batch_execute',
      args: {
        operations: [
          {
            tool_name: 'cortex_search_exact',
            parameters: {
              regex_pattern: searchToken,
              project_path: fixtureRoot,
              file_extension: 'rs',
            },
          },
          {
            tool_name: 'cortex_act_shell_exec',
            parameters: { command: 'pwd', cwd: repoRoot },
          },
        ],
        fail_fast: true,
      },
      reset: writeSearchFixture,
    },
    {
      id: 'cortex_mcp_hot_reload.restart',
      toolName: 'cortex_mcp_hot_reload',
      args: { reason: 'extension-self-test' },
    },
  ];
}