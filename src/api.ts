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
}

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
}

export interface MessageStats {
  durationMs?: number;
  timeToFirstTokenMs?: number;
  promptTokens?: number;
  completionTokens?: number;
  tokensPerSecond?: number;
  finishReason?: string;
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
  getOperator: () => invoke<OperatorSettings>('get_operator'),
  saveOperator: (settings: OperatorSettings) => invoke<OperatorSettings>('save_operator', { settings }),
  setLockdown: (on: boolean) => invoke<OperatorSettings>('set_lockdown', { on }),
  readActivity: (limit: number, kind: string, query: string) =>
    invoke<ActivityEntry[]>('read_activity', { limit, kind: kind || null, query: query || null }),
  requestDownload: (url: string) => invoke<Approval>('request_download', { url }),
  respondApproval: (id: string, approve: boolean, onProgress: (p: { done: number; total: number | null }) => void) => {
    const channel = new Channel<{ done: number; total: number | null }>();
    channel.onmessage = onProgress;
    return invoke<ApprovalOutcome>('respond_approval', { id, approve, onProgress: channel });
  },
};
