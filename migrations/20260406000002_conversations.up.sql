-- Conversation records for analytics and audit
CREATE TABLE conversations (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    session_id VARCHAR(255) NOT NULL,
    request_id VARCHAR(255) NOT NULL UNIQUE,
    query TEXT NOT NULL,
    response_text TEXT NOT NULL,
    locale VARCHAR(10) NOT NULL,
    intent VARCHAR(50) NOT NULL,
    meta JSONB NOT NULL DEFAULT '{}',
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_conversations_session ON conversations(session_id);
CREATE INDEX idx_conversations_request ON conversations(request_id);
CREATE INDEX idx_conversations_locale ON conversations(locale);
CREATE INDEX idx_conversations_intent ON conversations(intent);
CREATE INDEX idx_conversations_created ON conversations(created_at);
