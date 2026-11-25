-- Per-user Telegram binding. A user proves ownership of a chat by sending the
-- bot a one-time link code; until that round trip completes the row stays
-- unverified and never receives a notification. One binding per user, and a
-- chat may be claimed by only one account, so a verified chat can never be
-- hijacked by a second user guessing the same code.
CREATE TABLE IF NOT EXISTS public.user_telegram_bindings (
    id character varying(64) PRIMARY KEY,
    user_id character varying(64) NOT NULL REFERENCES public.users(id) ON DELETE CASCADE,
    chat_id character varying(64) UNIQUE,
    chat_type character varying(32),
    chat_title character varying(255),
    username character varying(255),
    link_code character varying(32) NOT NULL UNIQUE,
    link_code_expires_at timestamp with time zone NOT NULL,
    verified_at timestamp with time zone,
    last_notified_at timestamp with time zone,
    notification_count bigint NOT NULL DEFAULT 0,
    created_at timestamp with time zone NOT NULL DEFAULT NOW(),
    updated_at timestamp with time zone NOT NULL DEFAULT NOW()
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_user_telegram_bindings_user
  ON public.user_telegram_bindings USING btree (user_id);
