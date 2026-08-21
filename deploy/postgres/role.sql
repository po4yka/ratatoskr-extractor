-- Run as a PostgreSQL administrator before the first extractor start. Set the password separately
-- with `\password ratatoskr_extractor`; no credential belongs in this repository.

do $$
begin
    if not exists (select 1 from pg_roles where rolname = 'ratatoskr_extractor') then
        create role ratatoskr_extractor login;
    end if;
end
$$;

alter role ratatoskr_extractor nosuperuser nocreatedb nocreaterole noreplication nobypassrls;
grant connect, create on database ratatoskr to ratatoskr_extractor;

\connect ratatoskr
revoke all on schema public from ratatoskr_extractor;
