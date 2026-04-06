-- Reason categories for feedback
CREATE TABLE reason_categories (
    id SERIAL PRIMARY KEY,
    slug VARCHAR(100) UNIQUE NOT NULL,
    label VARCHAR(255) NOT NULL,
    is_active BOOLEAN NOT NULL DEFAULT TRUE
);

-- Seed default reason categories
INSERT INTO reason_categories (slug, label) VALUES
    ('factually_incorrect', 'Factually Incorrect'),
    ('missing_key_information', 'Missing Key Information'),
    ('ignored_or_refused_instructions', 'Ignored or Refused Instructions'),
    ('harmful_or_offensive', 'Harmful or Offensive');

-- User feedback on search responses
CREATE TABLE feedback (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    session_id VARCHAR(255) NOT NULL,
    request_id VARCHAR(255) NOT NULL,
    action VARCHAR(20) NOT NULL CHECK (action IN ('like', 'dislike', 'cancel')),
    reason_category_id INTEGER REFERENCES reason_categories(id),
    description TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_feedback_session ON feedback(session_id);
CREATE INDEX idx_feedback_request ON feedback(request_id);
CREATE INDEX idx_feedback_created ON feedback(created_at);

-- Popular/trending search queries (flushed from Redis)
CREATE TABLE popular_queries (
    id SERIAL PRIMARY KEY,
    locale VARCHAR(10) NOT NULL,
    query TEXT NOT NULL,
    search_count BIGINT NOT NULL DEFAULT 0,
    period VARCHAR(20) NOT NULL,
    period_date DATE NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE(locale, query, period, period_date)
);

CREATE INDEX idx_popular_queries_locale_count ON popular_queries(locale, search_count DESC);
CREATE INDEX idx_popular_queries_period ON popular_queries(period, period_date);
