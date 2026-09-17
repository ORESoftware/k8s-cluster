import "@typespec/json-schema";
using TypeSpec.JsonSchema;

namespace Ores.LocalK8s;

@id("RuntimeKind")
enum RuntimeKind {
  colimaKind: "colima-kind",
  dockerKind: "docker-kind",
  multipassK3s: "multipass-k3s",
}

@id("CpuArchitecture")
enum CpuArchitecture {
  arm64: "arm64",
  amd64: "amd64",
}

@id("ResourceClass")
enum ResourceClass {
  compact: "compact",
  standard: "standard",
  large: "large",
}

@id("ClusterTopology")
enum ClusterTopology {
  singleNode: "single-node",
  threeNode: "three-node",
}

@id("CloudMutationPolicy")
enum CloudMutationPolicy {
  denied: "denied",
}

@id("EvidenceSource")
enum EvidenceSource {
  githubActionsKind: "github-actions-kind",
  localKind: "local-kind",
}

@id("RuntimeProfile")
model RuntimeProfile {
  schema: string;
  name: string;
  runtime: RuntimeKind;
  architecture: CpuArchitecture;
  resourceClass: ResourceClass;
  topology: ClusterTopology;
  kubernetesVersion: string;
  runtimeArtifact: string;
  cloudMutationPolicy: CloudMutationPolicy;
  cloudWrites: boolean;
}

@id("RuntimeEvidence")
model RuntimeEvidence {
  schema: string;
  profileName: string;
  profileSha256: string;
  evidenceSource: EvidenceSource;
  sourceSha: string;
  runtime: RuntimeKind;
  architecture: CpuArchitecture;
  resourceClass: ResourceClass;
  topology: ClusterTopology;
  kubernetesVersion: string;
  runtimeArtifact: string;
  apiServerReady: boolean;
  serverAdmissionPassed: boolean;
  rbacListPods: boolean;
  rbacGetSecrets: boolean;
  providerWrites: boolean;
  secretInputs: boolean;
  cloudMutationPolicy: CloudMutationPolicy;
  cloudWrites: boolean;
}
