-- Usage records store the caller IP as free-form text inside request_metadata,
-- so it can hold values that are not parseable as inet (a spoofed forwarded
-- header, an address with a port, an empty string). The client_ip filter needs
-- CIDR containment, which requires an inet cast -- but a plain cast aborts the
-- whole query on the first malformed row. This helper casts and yields NULL
-- instead of raising, so one bad row can never break the admin usage list.
CREATE OR REPLACE FUNCTION public.aether_safe_inet(value text) RETURNS inet
    LANGUAGE plpgsql IMMUTABLE
    AS $$
BEGIN
    RETURN value::inet;
EXCEPTION WHEN OTHERS THEN
    RETURN NULL;
END;
$$;
