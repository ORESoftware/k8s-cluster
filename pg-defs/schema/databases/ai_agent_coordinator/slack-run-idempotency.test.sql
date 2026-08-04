\set ON_ERROR_STOP on

begin;

insert into ai_agent_coordinator.jobs (
  id,
  org,
  repo,
  task_type,
  payload,
  idempotency_key
) values (
  'slack-valid',
  'ORESoftware',
  'ai-agent-coordinator.rs',
  'slack_agent_run',
  '{"schema_version":1,"run_id":"ores-00112233445566778899aabb"}'::jsonb,
  'ores-00112233445566778899aabb'
);

-- Existing queue types keep their current optional-idempotency behavior.
insert into ai_agent_coordinator.jobs (
  id,
  org,
  repo,
  task_type,
  payload,
  idempotency_key
) values (
  'generic-valid',
  'ORESoftware',
  'ai-agent-coordinator.rs',
  'code_change',
  '{}'::jsonb,
  null
);

do $contract$
begin
  begin
    insert into ai_agent_coordinator.jobs (
      id, org, repo, task_type, payload, idempotency_key
    ) values (
      'slack-missing-key',
      'ORESoftware',
      'ai-agent-coordinator.rs',
      'slack_agent_run',
      '{"schema_version":1,"run_id":"ores-111122223333444455556666"}'::jsonb,
      null
    );
    raise exception 'expected missing Slack idempotency key to fail';
  exception
    when check_violation then null;
  end;

  begin
    insert into ai_agent_coordinator.jobs (
      id, org, repo, task_type, payload, idempotency_key
    ) values (
      'slack-mismatched-key',
      'ORESoftware',
      'ai-agent-coordinator.rs',
      'slack_agent_run',
      '{"schema_version":1,"run_id":"ores-222233334444555566667777"}'::jsonb,
      'ores-aaaaaaaaaaaaaaaaaaaaaaaa'
    );
    raise exception 'expected mismatched Slack idempotency key to fail';
  exception
    when check_violation then null;
  end;

  begin
    insert into ai_agent_coordinator.jobs (
      id, org, repo, task_type, payload, idempotency_key
    ) values (
      'slack-uppercase-key',
      'ORESoftware',
      'ai-agent-coordinator.rs',
      'slack_agent_run',
      '{"schema_version":1,"run_id":"ores-ABCDEFABCDEFABCDEFABCDEF"}'::jsonb,
      'ores-ABCDEFABCDEFABCDEFABCDEF'
    );
    raise exception 'expected noncanonical Slack run ID to fail';
  exception
    when check_violation then null;
  end;

  begin
    insert into ai_agent_coordinator.jobs (
      id, org, repo, task_type, payload, idempotency_key
    ) values (
      'slack-wrong-schema',
      'ORESoftware',
      'ai-agent-coordinator.rs',
      'slack_agent_run',
      '{"schema_version":2,"run_id":"ores-333344445555666677778888"}'::jsonb,
      'ores-333344445555666677778888'
    );
    raise exception 'expected unsupported Slack schema version to fail';
  exception
    when check_violation then null;
  end;

  begin
    insert into ai_agent_coordinator.jobs (
      id, org, repo, task_type, payload, idempotency_key
    ) values (
      'slack-array-payload',
      'ORESoftware',
      'ai-agent-coordinator.rs',
      'slack_agent_run',
      '[]'::jsonb,
      'ores-444455556666777788889999'
    );
    raise exception 'expected non-object Slack payload to fail';
  exception
    when check_violation then null;
  end;

  begin
    insert into ai_agent_coordinator.jobs (
      id, org, repo, task_type, payload, idempotency_key
    ) values (
      'slack-duplicate',
      'ORESoftware',
      'ai-agent-coordinator.rs',
      'slack_agent_run',
      '{"schema_version":1,"run_id":"ores-00112233445566778899aabb"}'::jsonb,
      'ores-00112233445566778899aabb'
    );
    raise exception 'expected duplicate Slack run ID to fail';
  exception
    when unique_violation then null;
  end;
end
$contract$;

do $assertions$
declare
  slack_count bigint;
  generic_count bigint;
begin
  select count(*) into slack_count
  from ai_agent_coordinator.jobs
  where task_type = 'slack_agent_run';

  select count(*) into generic_count
  from ai_agent_coordinator.jobs
  where task_type = 'code_change';

  if slack_count <> 1 then
    raise exception 'expected exactly one accepted Slack run, found %', slack_count;
  end if;
  if generic_count <> 1 then
    raise exception 'expected generic queue behavior to remain accepted';
  end if;
end
$assertions$;

rollback;
