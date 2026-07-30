use std::collections::BTreeMap;
use std::env;

use push_notification_server::{
    ContactContent, ContactJob, ContactOutcomeClass, ContactProvider, ContactProviderKind,
    ContactTarget, ContractVersion, SendGridConfig, SendGridProvider, SendGridRegion,
    TraceMetadata, TwilioConfig, TwilioCredentials, TwilioProvider, TwilioSender,
};

fn required_env(name: &str) -> String {
    env::var(name)
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| panic!("{name} is required for this ignored provider canary"))
}

fn contact_job(
    job_id: &str,
    provider: ContactProviderKind,
    target: ContactTarget,
    content: ContactContent,
) -> ContactJob {
    ContactJob {
        version: ContractVersion::V1,
        job_id: job_id.to_owned(),
        tenant_id: "provider-canary".to_owned(),
        application_id: "provider-canary".to_owned(),
        idempotency_key: format!("provider-canary-{job_id}"),
        provider,
        target,
        content,
        trace: TraceMetadata {
            correlation_id: Some(format!("provider-canary-{job_id}")),
            ..TraceMetadata::default()
        },
    }
}

/// Validates the exact SendGrid Mail Send request against the configured account
/// without delivering email. SendGrid sandbox mode returns validation results but
/// generates no delivery or Event Webhook activity.
#[tokio::test]
#[ignore = "requires SendGrid sandbox canary secrets"]
async fn sendgrid_sandbox_accepts_the_canonical_email_contract() {
    let api_key = required_env("SENDGRID_CANARY_API_KEY");
    let from_email = required_env("SENDGRID_CANARY_FROM_EMAIL");
    let to_email = required_env("SENDGRID_CANARY_TO_EMAIL");
    let region = match env::var("SENDGRID_CANARY_REGION")
        .unwrap_or_else(|_| "global".to_owned())
        .trim()
        .to_ascii_lowercase()
        .as_str()
    {
        "global" | "us" => SendGridRegion::Global,
        "eu" | "europe" => SendGridRegion::Europe,
        value => panic!("unsupported SENDGRID_CANARY_REGION: {value}"),
    };
    let config = SendGridConfig::new(
        api_key,
        from_email,
        Some("Provider Canary".to_owned()),
        region,
    )
    .expect("valid SendGrid canary configuration")
    .with_sandbox_mode(true);
    let provider = SendGridProvider::new(config).expect("SendGrid provider");
    let job = contact_job(
        "sendgrid-sandbox",
        ContactProviderKind::Sendgrid,
        ContactTarget::Email {
            address: to_email,
            name: Some("Canary Recipient".to_owned()),
        },
        ContactContent::Email {
            subject: Some("SendGrid sandbox contract canary".to_owned()),
            text: Some("This request is validated but never delivered.".to_owned()),
            html: Some("<p>This request is validated but never delivered.</p>".to_owned()),
            template_id: None,
            dynamic_template_data: BTreeMap::new(),
            reply_to: None,
        },
    );
    let outcome = provider.send(&job).await.expect("SendGrid canary request");
    assert_eq!(outcome.class, ContactOutcomeClass::Accepted);
    let serialized = serde_json::to_string(&outcome).expect("SendGrid outcome JSON");
    assert!(!serialized.contains(match &job.target {
        ContactTarget::Email { address, .. } => address,
        _ => unreachable!(),
    }));
}

/// Uses Twilio test credentials and the documented valid magic From number.
/// Test credentials return realistic API responses without billing the account,
/// altering production state, or connecting to a real carrier.
#[tokio::test]
#[ignore = "requires Twilio test credentials"]
async fn twilio_test_credentials_accept_the_canonical_sms_contract() {
    let account_sid = required_env("TWILIO_TEST_ACCOUNT_SID");
    let auth_token = required_env("TWILIO_TEST_AUTH_TOKEN");
    let to_number = required_env("TWILIO_TEST_TO_NUMBER");
    let config = TwilioConfig::new(
        account_sid,
        TwilioCredentials::AuthToken { token: auth_token },
        TwilioSender::PhoneNumber {
            e164: "+15005550006".to_owned(),
        },
    )
    .expect("valid Twilio test configuration");
    let provider = TwilioProvider::new(config).expect("Twilio provider");
    let job = contact_job(
        "twilio-test",
        ContactProviderKind::Twilio,
        ContactTarget::Sms { e164: to_number },
        ContactContent::Sms {
            body: "Twilio test-credential contract canary".to_owned(),
        },
    );
    let outcome = provider.send(&job).await.expect("Twilio test request");
    assert_eq!(outcome.class, ContactOutcomeClass::Accepted);
    let serialized = serde_json::to_string(&outcome).expect("Twilio outcome JSON");
    assert!(!serialized.contains(match &job.target {
        ContactTarget::Sms { e164 } => e164,
        _ => unreachable!(),
    }));
}
