import "@typespec/json-schema";
using TypeSpec.JsonSchema;

namespace BrowserMcpPlatformJobs;

@id("PlatformJobDomain")
enum PlatformJobDomain {
  greenhouse: "greenhouse.io",
  lever: "lever.co",
  ashby: "ashbyhq.com",
  myWorkdayJobs: "myworkdayjobs.com",
  workday: "workday.com",
  smartRecruiters: "smartrecruiters.com",
  icims: "icims.com",
  jobvite: "jobvite.com",
  workable: "workable.com",
  bambooHr: "bamboohr.com",
  recruitee: "recruitee.com",
  applyToJob: "applytojob.com",
  ripplingCandidate: "ats.rippling.com",
  breezy: "breezy.hr",
  jobscore: "jobscore.com",
  ceipalCandidateIn: "candidateportalin.ceipal.com",
  ceipalCandidateNew: "candidateportalnew.ceipal.com",
}

@id("PlatformJobBlockedDomain")
enum PlatformJobBlockedDomain {
  careersPage: "careers-page.com",
  appcastRedirector: "click.appcast.io",
  indeedRedirector: "to.indeed.com",
  indeedMarketplace: "www.indeed.com",
  zipRecruiterMarketplace: "www.ziprecruiter.com",
  linkedInMarketplace: "linkedin.com",
  googleDocs: "docs.google.com",
  googleStatic: "www.gstatic.com",
  googleSslStatic: "ssl.gstatic.com",
  googleFonts: "fonts.googleapis.com",
  googleFontStatic: "fonts.gstatic.com",
}

@id("GloballyBlockedDomain")
enum GloballyBlockedDomain {
  googleAccounts: "accounts.google.com",
  googleMail: "mail.google.com",
  microsoftLogin: "login.microsoftonline.com",
  paypal: "paypal.com",
  stripe: "stripe.com",
}

@id("HumanCompletionPoint")
enum HumanCompletionPoint {
  captcha: "captcha",
  mfa: "mfa",
}

@id("BlockedFieldClass")
enum BlockedFieldClass {
  ssn: "ssn",
  taxIdentifier: "tax_identifier",
  bankAccount: "bank_account",
  paymentCard: "payment_card",
  mfa: "mfa",
  otp: "otp",
  pin: "pin",
  credential: "credential",
  demographic: "demographic",
  disability: "disability",
  signature: "signature",
  legalAttestation: "legal_attestation",
  compensationCommitment: "compensation_commitment",
}

@id("BrowserWorkflowPolicy")
model BrowserWorkflowPolicy {
  workflowId: "platform-jobs";
  allowedDomains: PlatformJobDomain[];
  blockedPlatformJobDomains: PlatformJobBlockedDomain[];
  globallyBlockedDomains: GloballyBlockedDomain[];
  blockedFieldClasses: BlockedFieldClass[];
  humanCompletionPoints: HumanCompletionPoint[];
  requireExplicitApproval: true;
  requireRevisionBoundActionDigest: true;
  allowArbitraryCompanySites: false;
  allowMarketplaceNavigation: false;
  allowRedirectorNavigation: false;
  permitCaptchaAutomation: false;
}
