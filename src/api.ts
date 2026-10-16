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
};
