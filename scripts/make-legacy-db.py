"""Creates a database in the format of the earlier "Local Qwen" build, for the
migration end-to-end test. Usage: python scripts/make-legacy-db.py <path>"""
import json
import sqlite3
import sys
import time

path = sys.argv[1]
now = int(time.time() * 1000)
db = sqlite3.connect(path)
db.executescript(
    """
    CREATE TABLE conversations (id INTEGER PRIMARY KEY AUTOINCREMENT, title TEXT NOT NULL,
        model TEXT NOT NULL DEFAULT '', created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL);
    CREATE TABLE messages (id INTEGER PRIMARY KEY AUTOINCREMENT,
        conversation_id INTEGER NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
        role TEXT NOT NULL CHECK (role IN ('user', 'assistant')), content TEXT NOT NULL DEFAULT '',
        reasoning TEXT NOT NULL DEFAULT '', model TEXT NOT NULL DEFAULT '',
        status TEXT NOT NULL DEFAULT 'complete', error TEXT, stats TEXT, created_at INTEGER NOT NULL);
    CREATE INDEX idx_messages_conversation ON messages(conversation_id, id);
    CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);
    PRAGMA user_version = 1;
    """
)
db.execute("INSERT INTO conversations (title, model, created_at, updated_at) VALUES (?, ?, ?, ?)",
           ("Legacy chat about tomatoes", "qwen3.5:4b", now, now))
db.execute("INSERT INTO messages (conversation_id, role, content, model, status, created_at) VALUES (1, 'user', ?, 'qwen3.5:4b', 'complete', ?)",
           ("How often should I water tomatoes?", now))
db.execute("INSERT INTO messages (conversation_id, role, content, model, status, created_at) VALUES (1, 'assistant', ?, 'qwen3.5:4b', 'complete', ?)",
           ("About once or twice a week, deeply.", now))
settings = {"baseUrl": "http://127.0.0.1:11434/v1", "serverCommand": "ollama serve", "autoStartServer": True,
            "systemPrompt": "Legacy prompt: answer like a gardener.", "temperature": 0.65, "maxTokens": 8192,
            "defaultModel": "qwen3.5:4b"}
db.execute("INSERT INTO settings (key, value) VALUES ('app', ?)", (json.dumps(settings),))
db.commit()
db.close()
print(f"legacy database written to {path}")
