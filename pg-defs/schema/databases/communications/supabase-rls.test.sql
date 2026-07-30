\set ON_ERROR_STOP on

begin;

insert into communications.endpoints (
  id, tenant_id, application_id, shared_user_id, supabase_user_id,
  channel, provider, target_ciphertext, target_nonce, target_key_id,
  target_fingerprint, consent_state, verified_at
) values
  (
    '10000000-0000-0000-0000-000000000001', 'tenant-a', 'app-a',
    'shared-owner', '11111111-1111-1111-1111-111111111111',
    'email', 'sendgrid', decode('01', 'hex'), decode(repeat('01', 12), 'hex'),
    'communications-test-key', repeat('a', 64), 'granted', now()
  ),
  (
    '10000000-0000-0000-0000-000000000002', 'tenant-a', 'app-a',
    'shared-other', '22222222-2222-2222-2222-222222222222',
    'sms', 'twilio', decode('02', 'hex'), decode(repeat('02', 12), 'hex'),
    'communications-test-key', repeat('b', 64), 'granted', now()
  );

insert into communications.preferences (
  id, tenant_id, application_id, shared_user_id, supabase_user_id,
  purpose, channel_order
) values
  (
    '20000000-0000-0000-0000-000000000001', 'tenant-a', 'app-a',
    'shared-owner', '11111111-1111-1111-1111-111111111111',
    'security_alert', '["push","email","sms"]'::jsonb
  ),
  (
    '20000000-0000-0000-0000-000000000002', 'tenant-a', 'app-a',
    'shared-other', '22222222-2222-2222-2222-222222222222',
    'security_alert', '["push"]'::jsonb
  );

insert into communications.jobs (
  id, tenant_id, application_id, shared_user_id, supabase_user_id,
  purpose, idempotency_key, content_ciphertext, content_nonce,
  content_key_id, content_fingerprint, delivery_policy
) values
  (
    '30000000-0000-0000-0000-000000000001', 'tenant-a', 'app-a',
    'shared-owner', '11111111-1111-1111-1111-111111111111',
    'security_alert', 'owner-event-1', decode('03', 'hex'),
    decode(repeat('03', 12), 'hex'), 'communications-test-key', repeat('c', 64),
    '{"channel_order":["push","email"]}'::jsonb
  ),
  (
    '30000000-0000-0000-0000-000000000002', 'tenant-a', 'app-a',
    'shared-other', '22222222-2222-2222-2222-222222222222',
    'security_alert', 'other-event-1', decode('04', 'hex'),
    decode(repeat('04', 12), 'hex'), 'communications-test-key', repeat('d', 64),
    '{"channel_order":["sms"]}'::jsonb
  );

insert into communications.attempts (
  id, job_id, endpoint_id, attempt_number, channel, provider,
  request_fingerprint, state, outcome_class
) values
  (
    '40000000-0000-0000-0000-000000000001',
    '30000000-0000-0000-0000-000000000001',
    '10000000-0000-0000-0000-000000000001',
    1, 'email', 'sendgrid', repeat('e', 64), 'accepted', 'accepted'
  ),
  (
    '40000000-0000-0000-0000-000000000002',
    '30000000-0000-0000-0000-000000000002',
    '10000000-0000-0000-0000-000000000002',
    1, 'sms', 'twilio', repeat('f', 64), 'accepted', 'accepted'
  );

insert into communications.receipts (
  id, attempt_id, provider, provider_event_id, provider_message_id,
  event_type, normalized_state, terminal, payload_sha256,
  signature_verified
) values
  (
    '50000000-0000-0000-0000-000000000001',
    '40000000-0000-0000-0000-000000000001',
    'sendgrid', 'sg-owner-event', 'sg-owner-message', 'delivered',
    'delivered', true, repeat('1', 64), true
  ),
  (
    '50000000-0000-0000-0000-000000000002',
    '40000000-0000-0000-0000-000000000002',
    'twilio', 'twilio-other-event', 'SM-other-message', 'delivered',
    'delivered', true, repeat('2', 64), true
  );

set local role authenticated;
select set_config(
  'request.jwt.claims',
  '{"sub":"11111111-1111-1111-1111-111111111111","shared_user_id":"shared-owner","role":"authenticated"}',
  true
);

do $$
declare
  visible_count integer;
  affected_count integer;
begin
  select count(*) into visible_count
  from communications.preferences;
  if visible_count <> 1 then
    raise exception 'owner should see exactly one preference, saw %', visible_count;
  end if;

  select count(*) into visible_count
  from communications.endpoint_summaries;
  if visible_count <> 1 then
    raise exception 'owner should see exactly one endpoint summary, saw %', visible_count;
  end if;

  select count(*) into visible_count
  from communications.user_communication_history;
  if visible_count <> 1 then
    raise exception 'owner should see exactly one communication attempt, saw %', visible_count;
  end if;

  update communications.preferences
  set locale = 'en-US'
  where shared_user_id = 'shared-other';
  get diagnostics affected_count = row_count;
  if affected_count <> 0 then
    raise exception 'owner updated another user preference';
  end if;

  begin
    perform count(*) from communications.endpoints;
    raise exception 'authenticated role unexpectedly read endpoint ciphertext table';
  exception
    when insufficient_privilege then null;
  end;

  begin
    insert into communications.preferences (
      tenant_id, application_id, shared_user_id, supabase_user_id,
      purpose, channel_order
    ) values (
      'tenant-a', 'app-a', 'shared-other',
      '22222222-2222-2222-2222-222222222222',
      'billing_notice', '["email"]'::jsonb
    );
    raise exception 'owner inserted a preference for another user';
  exception
    when insufficient_privilege then null;
  end;
end
$$;

rollback;
