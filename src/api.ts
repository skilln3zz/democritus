// Typed wrappers around the Rust commands in src-tauri/src/main.rs.
import { Channel, invoke } from '@tauri-apps/api/core';

export interface Settings {
  baseUrl: string;
  serverCommand: string;
  autoStartServer: boolean;
  systemPrompt: string;
  temperature: number;
  maxTokens: number;
  defaultModel: string;
  homeDir: string;
  memoryHelperModel: string;
  activeProject: string;
  contextTokens: number;
  /** 'system' (follow Windows), 'light' or 'dark'. */
  theme: string;
  /** How a reply shows its thinking. */
  thinkingDisplay: ThinkingDisplay;
}

export type ThinkingDisplay = 'orb' | 'thoughts' | 'both';

export interface ModelInfo {
  id: string;
  sizeBytes: number | null;
  parameterSize: string | null;
  quantization: string | null;
}

export interface ServerStatus {
  state: 'connected' | 'starting' | 'disconnected';
  startedByApp: boolean;
  baseUrl: string;
  error: string | null;
}

export interface Conversation {
  id: number;
  title: string;
  model: string;
  createdAt: number;
  updatedAt: number;
  project: string;
}

export interface MessageStats {
  durationMs?: number;
  timeToFirstTokenMs?: number;
  promptTokens?: number;
  completionTokens?: number;
  tokensPerSecond?: number;
  finishReason?: string;
  context?: ContextReport;
  /** From the first thinking token until the answer started. */
  thinkingMs?: number;
  /** Thinking pieces received (about one token each). */
  thoughts?: number;
}

/** What went into a request (saved with each reply). */
export interface ContextReport {
  numCtx: number;
  estimatedPromptTokens: number;
  memoryFiles: string[];
  searchHits: number;
  historyTurns: number;
  hasSummary: boolean;
  omittedTurns: number;
  clipped: boolean;
  switchedFrom: string | null;
}

export interface LastTime {
  project: string;
  lastTask: string;
  nextStep: string;
  conversationId: number | null;
  conversationTitle: string;
  updatedAt: number;
}

export interface Message {
  id: number;
  conversationId: number;
  role: 'user' | 'assistant';
  content: string;
  reasoning: string;
  model: string;
  status: 'complete' | 'streaming' | 'stopped' | 'error';
  error: string | null;
  stats: MessageStats | null;
  createdAt: number;
}

export type StreamEvent =
  | { type: 'started'; userMessage: Message; assistantMessageId: number }
  | { type: 'status'; text: string }
  | { type: 'reasoning'; text: string }
  | { type: 'content'; text: string }
  | { type: 'reclassify' }
  | { type: 'done'; message: Message };

export interface LoadedModel {
  name: string;
  sizeBytes: number;
  vramBytes: number;
  contextLength: number | null;
  expiresAt: string | null;
}

export interface MemoryInfo {
  totalRam: number;
  freeRam: number;
  gpuName: string | null;
  totalVram: number | null;
  freeVram: number | null;
}

export interface FitReport {
  model: string;
  alreadyLoaded: boolean;
  neededBytes: number;
  neededIsEstimate: boolean;
  freeBytes: number;
  others: LoadedModel[];
  fitsNow: boolean;
  fitsAfterUnloadingOthers: boolean;
  memory: MemoryInfo;
}

export interface OperatorSettings {
  lockdown: boolean;
  allowDownloads: boolean;
  downloadAllowlist: string[];
  autoApplyMemory: boolean;
  memoryPaused: boolean;
  allowAutoModelSelect: boolean;
  allowTraining: boolean;
}

export interface AppPaths {
  home: string;
  memory: string;
  logs: string;
  training: string;
  downloads: string;
}

export interface ActivityEntry {
  ts: number;
  time: string;
  kind: string;
  action: string;
  detail: unknown;
}

export interface Approval {
  id: string;
  kind: 'lookup' | 'download';
  url: string;
  fileName: string;
  sizeBytes: number | null;
  sha256: string | null;
  servedFrom: string | null;
  createdMs: number;
}

export interface DownloadResult {
  path: string;
  sizeBytes: number;
  sha256: string;
  checksumMatches: boolean | null;
}

export type ApprovalOutcome =
  | { type: 'denied' }
  | { type: 'needsApproval'; approval: Approval }
  | { type: 'downloaded'; result: DownloadResult };

export interface MemoryFile {
  path: string;
  title: string;
  description: string;
  size: number;
  modifiedMs: number;
  core: boolean;
}

export interface MemoryOverview {
  memoryDir: string;
  files: MemoryFile[];
  paused: boolean;
  lockdown: boolean;
  autoApply: boolean;
  pending: number;
  queued: number;
  semanticInstalled: boolean;
  semanticFileDownloaded: boolean;
  chunks: number;
  embeddedChunks: number;
  activeProject: string;
  helperModel: string;
}

export interface MemoryHit {
  path: string;
  heading: string;
  text: string;
  score: number;
}

export interface Snapshot {
  id: string;
  time: string;
  size: number;
}

export interface MemoryOp {
  file: string;
  section: string | null;
  action: string;
  content: string;
}

export interface Proposal {
  id: number;
  conversationId: number | null;
  conversationTitle: string;
  status: string;
  model: string;
  ops: MemoryOp[];
  rejected: { op: MemoryOp; reason: string }[];
  project: string;
  note: string;
  error: string | null;
  createdAt: number;
}

export interface FilePreview {
  path: string;
  before: string;
  after: string;
}

export interface ProjectInfo {
  slug: string;
  title: string;
}

export interface SecurityFix {
  id: string;
  label: string;
  /** The exact commands that will run (empty for in-app fixes like Lockdown). */
  commands: string[];
  /** Runs in an elevated PowerShell (Windows asks for administrator rights). */
  admin: boolean;
  note: string;
}

export interface SecurityItem {
  id: string;
  title: string;
  status: 'ok' | 'bad' | 'unknown' | 'info';
  detail: string;
  fix: SecurityFix | null;
}

export interface DownloadProgress {
  done: number;
  total: number | null;
  /** Set while switching the firewall rule; null during the transfer. */
  stage?: string | null;
}

export interface FirewallStatus {
  state: 'none' | 'on' | 'off' | 'unknown';
  detail: string;
}

export interface SendRequest {
  requestId: string;
  conversationId: number;
  model: string;
  content: string;
}

export const api = {
  getSettings: () => invoke<Settings>('get_settings'),
  saveSettings: (settings: Settings) => invoke<Settings>('save_settings', { settings }),
  listModels: () => invoke<ModelInfo[]>('list_models'),
  serverStatus: () => invoke<ServerStatus>('server_status'),
  startServer: () => invoke<ServerStatus>('start_server'),
  loadedModels: () => invoke<LoadedModel[]>('loaded_models'),
  unloadModel: (model: string) => invoke<void>('unload_model', { model }),
  memoryInfo: () => invoke<MemoryInfo>('memory_info'),
  modelFit: (model: string) => invoke<FitReport | null>('model_fit', { model }),
  modelSpeeds: () => invoke<Record<string, number>>('model_speeds'),
  listConversations: (query: string) => invoke<Conversation[]>('list_conversations', { query: query || null }),
  createConversation: (title: string, model: string) => invoke<Conversation>('create_conversation', { title, model }),
  renameConversation: (id: number, title: string) => invoke<void>('rename_conversation', { id, title }),
  deleteConversation: (id: number) => invoke<void>('delete_conversation', { id }),
  setConversationModel: (id: number, model: string) => invoke<void>('set_conversation_model', { id, model }),
  getMessages: (conversationId: number) => invoke<Message[]>('get_messages', { conversationId }),
  sendMessage: (req: SendRequest, onEvent: (e: StreamEvent) => void) => {
    const channel = new Channel<StreamEvent>();
    channel.onmessage = onEvent;
    return invoke<void>('send_message', { req, onEvent: channel });
  },
  stopGeneration: (requestId: string) => invoke<void>('stop_generation', { requestId }),
  appPaths: () => invoke<AppPaths>('app_paths'),
  memoryOverview: () => invoke<MemoryOverview>('memory_overview'),
  memoryRead: (path: string) => invoke<string>('memory_read', { path }),
  memoryWrite: (path: string, content: string) => invoke<void>('memory_write', { path, content }),
  memoryDelete: (path: string) => invoke<void>('memory_delete', { path }),
  memoryHistory: (path: string) => invoke<Snapshot[]>('memory_history', { path }),
  memorySnapshot: (path: string, id: string) => invoke<string>('memory_snapshot', { path, id }),
  memoryRestore: (path: string, id: string) => invoke<void>('memory_restore', { path, id }),
  memorySearch: (query: string) => invoke<MemoryHit[]>('memory_search', { query }),
  memoryProposals: (status: string) => invoke<Proposal[]>('memory_proposals', { status: status || null }),
  memoryPreview: (id: number) => invoke<FilePreview[]>('memory_preview', { id }),
  memoryApply: (id: number, selected: number[] | null) => invoke<number>('memory_apply', { id, selected }),
  memoryReject: (id: number) => invoke<void>('memory_reject', { id }),
  memoryKick: () => invoke<void>('memory_kick'),
  memoryInstallSemantic: () => invoke<void>('memory_install_semantic'),
  listProjects: () => invoke<ProjectInfo[]>('list_projects'),
  createProject: (name: string) => invoke<string>('create_project', { name }),
  setActiveProject: (slug: string, conversationId: number | null) => invoke<void>('set_active_project', { slug, conversationId }),
  resetLearning: () => invoke<string>('reset_learning'),
  lastTime: () => invoke<LastTime | null>('last_time'),
  getOperator: () => invoke<OperatorSettings>('get_operator'),
  saveOperator: (settings: OperatorSettings) => invoke<OperatorSettings>('save_operator', { settings }),
  setLockdown: (on: boolean) => invoke<OperatorSettings>('set_lockdown', { on }),
  securityCheck: () => invoke<SecurityItem[]>('security_check'),
  securityFix: (id: string) => invoke<void>('security_fix', { id }),
  readActivity: (limit: number, kind: string, query: string) =>
    invoke<ActivityEntry[]>('read_activity', { limit, kind: kind || null, query: query || null }),
  requestDownload: (url: string) => invoke<Approval>('request_download', { url }),
  firewallStatus: () => invoke<FirewallStatus>('firewall_status'),
  firewallReenable: () => invoke<FirewallStatus>('firewall_reenable'),
  respondApproval: (id: string, approve: boolean, onProgress: (p: DownloadProgress) => void) => {
    const channel = new Channel<DownloadProgress>();
    channel.onmessage = onProgress;
    return invoke<ApprovalOutcome>('respond_approval', { id, approve, onProgress: channel });
  },
};
