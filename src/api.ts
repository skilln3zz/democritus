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
  /** Context window for the coding agent. */
  agentContextTokens: number;
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
  /** The operator's rating: 1 thumbs up, -1 thumbs down, 0 none. */
  rating: number;
  /** The operator's corrected answer, shown and sent instead of the original. */
  edited: string | null;
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
  allowCodingAgent: boolean;
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

/** A change the coding agent wants to make, waiting for approval. */
export interface PlannedEdit {
  path: string;
  before: string;
  after: string;
  newFile: boolean;
}

export type AgentEvent =
  | { type: 'started'; taskId: string; folder: string; numCtx: number }
  | { type: 'step'; step: number }
  | { type: 'thinking'; text: string }
  | { type: 'content'; text: string }
  | { type: 'toolCall'; callId: string; name: string; summary: string }
  | { type: 'toolResult'; callId: string; ok: boolean; summary: string }
  | { type: 'approval'; approvalId: string; callId: string; edit: PlannedEdit }
  | { type: 'resolved'; approvalId: string; applied: boolean; message: string }
  | { type: 'done'; status: 'complete' | 'stopped' | 'limit' | 'error'; message: string; editsApplied: number; tokens: number };

export interface AgentRunRequest {
  taskId: string;
  folder: string;
  model: string;
  message: string;
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

/** A rated or edited reply that can become a lesson (Step 6D). */
export interface TrainingExample {
  messageId: number;
  conversationId: number;
  conversationTitle: string;
  model: string;
  rating: number;
  edited: boolean;
  excluded: boolean;
  prompt: string;
  answer: string;
  createdAt: number;
}

export interface ExportManifest {
  file: string;
  exportedAt: string;
  rated: number;
  edited: number;
  identity: number;
  excluded: number;
}

export interface DatasetStatus {
  folder: string;
  latest: ExportManifest | null;
  identityExamples: number;
}

export type BuildStage = 'preflight' | 'lessons' | 'training' | 'packaging' | 'testing' | 'done';
export type BuildStatus = 'running' | 'paused' | 'failed' | 'cancelled' | 'done';

export interface BuildCheck {
  id: string;
  label: string;
  ok: boolean;
  detail: string;
}

export interface BuildScores {
  identity: number;
  rules: number;
  everyday: number;
  coding: number;
  studentTps: number;
  teacherTps: number;
  speedup: number;
  testedAt: number;
}

export interface Build {
  id: string;
  name: string;
  testRun: boolean;
  student: string;
  teacher: string;
  lessonTarget: number;
  steps: number;
  gradAccum: number;
  stage: BuildStage;
  status: BuildStatus;
  statusText: string;
  pauseReason: string | null;
  createdAt: number;
  finishedAt: number | null;
  lessonsDone: number;
  lessonsFromYou: number;
  step: number;
  loss: number | null;
  lossStart: number | null;
  secondsPerStep: number | null;
  secondsPerLesson: number | null;
  checkpointAt: number | null;
  vramPeak: number | null;
  preflight: BuildCheck[];
  scores: BuildScores | null;
  error: string | null;
  log: { ts: number; text: string; alert: boolean }[];
  progress: number;
  eta: number | null;
}

export interface BuildSummary {
  id: string;
  name: string;
  testRun: boolean;
  teacher: string;
  finishedAt: number | null;
  scores: BuildScores | null;
  lessons: number;
}

export interface BuildOverview {
  current: Build | null;
  finished: BuildSummary[];
  trainingDir: string;
  students: string[];
  defaultTeacher: string;
}

export interface GpuStats {
  name: string;
  temp: number;
  memUsed: number;
  memTotal: number;
  util: number;
}

export interface BuildReport {
  scores: BuildScores;
  items: { category: string; prompt: string; student: string; teacher: string; studentScore: number; teacherScore: number }[];
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
  agentRun: (req: AgentRunRequest, onEvent: (e: AgentEvent) => void) => {
    const channel = new Channel<AgentEvent>();
    channel.onmessage = onEvent;
    return invoke<void>('agent_run', { req, onEvent: channel });
  },
  agentRespond: (approvalId: string, approve: boolean, note: string) => invoke<void>('agent_respond', { approvalId, approve, note }),
  agentStop: () => invoke<void>('agent_stop'),
  agentForget: (taskId: string) => invoke<void>('agent_forget', { taskId }),
  securityCheck: () => invoke<SecurityItem[]>('security_check'),
  securityFix: (id: string) => invoke<void>('security_fix', { id }),
  readActivity: (limit: number, kind: string, query: string) =>
    invoke<ActivityEntry[]>('read_activity', { limit, kind: kind || null, query: query || null }),
  requestDownload: (url: string) => invoke<Approval>('request_download', { url }),
  firewallStatus: () => invoke<FirewallStatus>('firewall_status'),
  firewallReenable: () => invoke<FirewallStatus>('firewall_reenable'),
  rateMessage: (id: number, rating: number) => invoke<void>('rate_message', { id, rating }),
  editReply: (id: number, text: string) => invoke<void>('edit_reply', { id, text }),
  setExampleExcluded: (id: number, excluded: boolean) => invoke<void>('set_example_excluded', { id, excluded }),
  trainingExamples: () => invoke<TrainingExample[]>('training_examples'),
  datasetStatus: () => invoke<DatasetStatus>('dataset_status'),
  exportDataset: () => invoke<ExportManifest>('export_dataset'),
  buildOverview: () => invoke<BuildOverview>('build_overview'),
  buildPreflight: (teacher: string, testRun: boolean) => invoke<BuildCheck[]>('build_preflight', { teacher, testRun }),
  buildStart: (teacher: string, testRun: boolean, lessons: number, useHistory: boolean) => invoke<Build>('build_start', { teacher, testRun, lessons, useHistory }),
  buildPause: () => invoke<void>('build_pause'),
  buildResume: () => invoke<void>('build_resume'),
  buildCancel: () => invoke<void>('build_cancel'),
  buildDismiss: () => invoke<void>('build_dismiss'),
  buildGpu: () => invoke<GpuStats | null>('build_gpu'),
  buildReport: (id: string) => invoke<BuildReport>('build_report', { id }),
  buildMakeDefault: (model: string) => invoke<void>('build_make_default', { model }),
  respondApproval: (id: string, approve: boolean, onProgress: (p: DownloadProgress) => void) => {
    const channel = new Channel<DownloadProgress>();
    channel.onmessage = onProgress;
    return invoke<ApprovalOutcome>('respond_approval', { id, approve, onProgress: channel });
  },
};
