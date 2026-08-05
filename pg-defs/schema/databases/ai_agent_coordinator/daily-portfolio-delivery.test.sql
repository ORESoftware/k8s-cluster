\set ON_ERROR_STOP on

begin;

insert into ai_agent_coordinator.daily_portfolio_delivery_runs (
  run_key,
  scheduled_run_key,
  mode,
  source_digest,
  plan_digest,
  delivery_digest,
  destination,
  idempotency_key
) values (
  'daily-portfolio:scheduled:2026-08-05',
  'daily-portfolio:scheduled:2026-08-05',
  'scheduled',
  repeat('a', 64),
  repeat('b', 64),
  repeat('c', 64),
  'slack:C0PORTFOLIO',
  'daily-portfolio:scheduled:2026-08-05'
);

insert into ai_agent_coordinator.daily_portfolio_delivery_runs (
  run_key,
  scheduled_run_key,
  mode,
  source_digest,
  plan_digest,
  delivery_digest,
  destination,
  idempotency_key,
  status,
  generation,
  attempts,
  last_error
) values (
  'daily-portfolio:recovery:2026-08-05:attempt-1',
  'daily-portfolio:scheduled:2026-08-05',
  'recovery',
  repeat('d', 64),
  repeat('e', 64),
  repeat('f', 64),
  'slack:C0PORTFOLIO',
  'daily-portfolio:recovery:2026-08-05:attempt-1',
  'failed',
  2,
  1,
  'bounded delivery timeout'
);

insert into ai_agent_coordinator.daily_portfolio_delivery_runs (
  run_key,
  scheduled_run_key,
  mode,
  source_digest,
  plan_digest,
  delivery_digest,
  destination,
  idempotency_key,
  status,
  generation,
  attempts,
  receipt_id,
  receipt_destination,
  receipt_body_digest,
  delivered_at
) values (
  'daily-portfolio:manual:operator-check',
  'daily-portfolio:scheduled:2026-08-05',
  'manual',
  repeat('1', 64),
  repeat('2', 64),
  repeat('3', 64),
  'slack:C0PORTFOLIO',
  'daily-portfolio:manual:operator-check',
  'delivered',
  2,
  1,
  'manual-receipt',
  'slack:C0PORTFOLIO',
  repeat('3', 64),
  now()
);

update ai_agent_coordinator.daily_portfolio_delivery_runs
set status = 'delivering',
    generation = 1,
    attempts = 1,
    lease_owner = 'worker-a',
    lease_fence = nextval('ai_agent_coordinator.daily_portfolio_delivery_fence_seq'),
    lease_expires_at = now() + interval '10 minutes',
    updated_at = now()
where run_key = 'daily-portfolio:scheduled:2026-08-05'
  and status = 'planned'
  and generation = 0;

update ai_agent_coordinator.daily_portfolio_delivery_runs
set status = 'delivered',
    generation = 2,
    receipt_id = 'scheduled-receipt',
    receipt_destination = destination,
    receipt_body_digest = delivery_digest,
    delivered_at = now(),
    lease_owner = null,
    lease_fence = null,
    lease_expires_at = null,
    updated_at = now()
where run_key = 'daily-portfolio:scheduled:2026-08-05'
  and status = 'delivering'
  and generation = 1;

insert into ai_agent_coordinator.daily_portfolio_delivery_baseline (
  singleton_key,
  source_run_key,
  scheduled_run_key,
  plan_digest,
  delivery_digest,
  receipt_id,
  delivered_at
) values (
  'scheduled',
  'daily-portfolio:scheduled:2026-08-05',
  'daily-portfolio:scheduled:2026-08-05',
  repeat('b', 64),
  repeat('c', 64),
  'scheduled-receipt',
  now()
);

do $contract$
begin
  begin
    insert into ai_agent_coordinator.daily_portfolio_delivery_runs (
      run_key, scheduled_run_key, mode, source_digest, plan_digest,
      delivery_digest, destination, idempotency_key
    ) values (
      'daily-portfolio:scheduled:2026-08-06',
      'daily-portfolio:scheduled:2026-08-05',
      'scheduled', repeat('a', 64), repeat('b', 64), repeat('c', 64),
      'slack:C0PORTFOLIO', 'daily-portfolio:scheduled:2026-08-06'
    );
    raise exception 'expected scheduled identity drift to fail';
  exception when check_violation then null;
  end;

  begin
    insert into ai_agent_coordinator.daily_portfolio_delivery_runs (
      run_key, scheduled_run_key, mode, source_digest, plan_digest,
      delivery_digest, destination, idempotency_key
    ) values (
      'daily-portfolio:manual:preview',
      'daily-portfolio:scheduled:2026-08-05',
      'manual', repeat('A', 64), repeat('b', 64), repeat('c', 64),
      'slack:C0PORTFOLIO', 'daily-portfolio:manual:preview'
    );
    raise exception 'expected uppercase digest to fail';
  exception when check_violation then null;
  end;

  begin
    insert into ai_agent_coordinator.daily_portfolio_delivery_runs (
      run_key, scheduled_run_key, mode, source_digest, plan_digest,
      delivery_digest, destination, idempotency_key
    ) values (
      'daily-portfolio:manual:operator-check',
      'daily-portfolio:scheduled:2026-08-05',
      'manual', repeat('1', 64), repeat('2', 64), repeat('3', 64),
      'slack:C0PORTFOLIO', 'daily-portfolio:manual:operator-check'
    );
    raise exception 'expected duplicate idempotency key to fail';
  exception when unique_violation then null;
  end;

  begin
    insert into ai_agent_coordinator.daily_portfolio_delivery_runs (
      run_key, scheduled_run_key, mode, source_digest, plan_digest,
      delivery_digest, destination, idempotency_key, status, generation,
      attempts
    ) values (
      'daily-portfolio:recovery:2026-08-06:no-lease',
      'daily-portfolio:scheduled:2026-08-06',
      'recovery', repeat('1', 64), repeat('2', 64), repeat('3', 64),
      'slack:C0PORTFOLIO', 'daily-portfolio:recovery:2026-08-06:no-lease',
      'delivering', 1, 1
    );
    raise exception 'expected delivering without a lease to fail';
  exception when check_violation then null;
  end;

  begin
    insert into ai_agent_coordinator.daily_portfolio_delivery_runs (
      run_key, scheduled_run_key, mode, source_digest, plan_digest,
      delivery_digest, destination, idempotency_key, lease_owner
    ) values (
      'daily-portfolio:manual:partial-lease',
      'daily-portfolio:scheduled:2026-08-06',
      'manual', repeat('1', 64), repeat('2', 64), repeat('3', 64),
      'slack:C0PORTFOLIO', 'daily-portfolio:manual:partial-lease', 'worker-a'
    );
    raise exception 'expected partial lease state to fail';
  exception when check_violation then null;
  end;

  begin
    insert into ai_agent_coordinator.daily_portfolio_delivery_runs (
      run_key, scheduled_run_key, mode, source_digest, plan_digest,
      delivery_digest, destination, idempotency_key, status, generation,
      attempts
    ) values (
      'daily-portfolio:recovery:2026-08-06:no-error',
      'daily-portfolio:scheduled:2026-08-06',
      'recovery', repeat('1', 64), repeat('2', 64), repeat('3', 64),
      'slack:C0PORTFOLIO', 'daily-portfolio:recovery:2026-08-06:no-error',
      'failed', 2, 1
    );
    raise exception 'expected failed state without bounded error to fail';
  exception when check_violation then null;
  end;

  begin
    insert into ai_agent_coordinator.daily_portfolio_delivery_runs (
      run_key, scheduled_run_key, mode, source_digest, plan_digest,
      delivery_digest, destination, idempotency_key, status, generation,
      attempts
    ) values (
      'daily-portfolio:manual:no-receipt',
      'daily-portfolio:scheduled:2026-08-06',
      'manual', repeat('1', 64), repeat('2', 64), repeat('3', 64),
      'slack:C0PORTFOLIO', 'daily-portfolio:manual:no-receipt',
      'delivered', 2, 1
    );
    raise exception 'expected delivered state without receipt to fail';
  exception when check_violation then null;
  end;

  begin
    insert into ai_agent_coordinator.daily_portfolio_delivery_runs (
      run_key, scheduled_run_key, mode, source_digest, plan_digest,
      delivery_digest, destination, idempotency_key, status, generation,
      attempts, receipt_id, receipt_destination, receipt_body_digest,
      delivered_at
    ) values (
      'daily-portfolio:manual:mismatched-receipt',
      'daily-portfolio:scheduled:2026-08-06',
      'manual', repeat('1', 64), repeat('2', 64), repeat('3', 64),
      'slack:C0PORTFOLIO', 'daily-portfolio:manual:mismatched-receipt',
      'delivered', 2, 1, 'wrong-receipt', 'slack:OTHER', repeat('4', 64),
      now()
    );
    raise exception 'expected mismatched receipt to fail';
  exception when check_violation then null;
  end;

  begin
    insert into ai_agent_coordinator.daily_portfolio_delivery_baseline (
      singleton_key, source_run_key, scheduled_run_key, plan_digest,
      delivery_digest, receipt_id, delivered_at
    ) values (
      'other', 'daily-portfolio:manual:operator-check',
      'daily-portfolio:scheduled:2026-08-05', repeat('2', 64),
      repeat('3', 64), 'manual-receipt', now()
    );
    raise exception 'expected noncanonical singleton baseline key to fail';
  exception when check_violation then null;
  end;
end
$contract$;

do $assertions$
declare
  scheduled_status text;
  scheduled_generation bigint;
  scheduled_attempts bigint;
  baseline_count bigint;
  first_fence bigint;
  second_fence bigint;
begin
  select status, generation, attempts
    into scheduled_status, scheduled_generation, scheduled_attempts
  from ai_agent_coordinator.daily_portfolio_delivery_runs
  where run_key = 'daily-portfolio:scheduled:2026-08-05';

  if scheduled_status <> 'delivered'
     or scheduled_generation <> 2
     or scheduled_attempts <> 1 then
    raise exception 'scheduled delivery did not reach the exact terminal state';
  end if;

  select count(*) into baseline_count
  from ai_agent_coordinator.daily_portfolio_delivery_baseline;
  if baseline_count <> 1 then
    raise exception 'expected exactly one scheduled baseline row, found %', baseline_count;
  end if;

  first_fence := nextval('ai_agent_coordinator.daily_portfolio_delivery_fence_seq');
  second_fence := nextval('ai_agent_coordinator.daily_portfolio_delivery_fence_seq');
  if second_fence <= first_fence then
    raise exception 'fencing sequence did not advance monotonically';
  end if;
end
$assertions$;

rollback;
