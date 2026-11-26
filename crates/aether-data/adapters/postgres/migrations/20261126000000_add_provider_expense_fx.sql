-- Provider expenses keep the amount exactly as paid.  The three columns below
-- snapshot the display currency in force and the rate that converted it, so a
-- recorded row never drifts when the operator edits the rate table later.
ALTER TABLE public.provider_expenses
    ADD COLUMN IF NOT EXISTS fx_rate_used numeric(18,8),
    ADD COLUMN IF NOT EXISTS amount_base numeric(20,8),
    ADD COLUMN IF NOT EXISTS base_currency varchar(3);

DO $$
BEGIN
    IF NOT EXISTS (
        SELECT 1 FROM pg_constraint WHERE conname = 'provider_expenses_base_currency_check'
    ) THEN
        ALTER TABLE public.provider_expenses
            ADD CONSTRAINT provider_expenses_base_currency_check
            CHECK (base_currency IS NULL OR base_currency ~ '^[A-Z]{3}$');
    END IF;
    IF NOT EXISTS (
        SELECT 1 FROM pg_constraint WHERE conname = 'provider_expenses_fx_rate_positive_check'
    ) THEN
        ALTER TABLE public.provider_expenses
            ADD CONSTRAINT provider_expenses_fx_rate_positive_check
            CHECK (fx_rate_used IS NULL OR fx_rate_used > 0);
    END IF;
    IF NOT EXISTS (
        SELECT 1 FROM pg_constraint WHERE conname = 'provider_expenses_amount_base_positive_check'
    ) THEN
        ALTER TABLE public.provider_expenses
            ADD CONSTRAINT provider_expenses_amount_base_positive_check
            CHECK (amount_base IS NULL OR amount_base > 0);
    END IF;
END $$;

-- Base-currency rollups group over a single snapshot currency, but the column
-- stays nullable so rows recorded before this migration keep filtering out of
-- the converted totals instead of being coerced onto a guessed rate.
CREATE INDEX IF NOT EXISTS ix_provider_expenses_base_currency
    ON public.provider_expenses(base_currency)
    WHERE base_currency IS NOT NULL;
