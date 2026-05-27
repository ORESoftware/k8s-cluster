package v1alpha1

import (
	corev1 "k8s.io/api/core/v1"
	"k8s.io/apimachinery/pkg/api/resource"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
)

// ThreadDesiredState is the operator-facing knob for sleep/wake.
//
// +kubebuilder:validation:Enum=Running;Sleeping
type ThreadDesiredState string

const (
	// ThreadDesiredStateRunning asks the operator to keep one pod up.
	ThreadDesiredStateRunning ThreadDesiredState = "Running"
	// ThreadDesiredStateSleeping asks the operator to scale to zero
	// while keeping the workspace PVC intact (mirrors the existing
	// idle-reaper "scale to 0, keep PVC" sleep primitive).
	ThreadDesiredStateSleeping ThreadDesiredState = "Sleeping"
)

// ThreadPhase mirrors the lifecycle states surfaced by the existing
// /u/admin/k8s dashboard so a Thread CR's phase is directly comparable
// to the dashboard's derived counters.
//
// +kubebuilder:validation:Enum=Pending;Active;Sleeping;Failed;Terminating
type ThreadPhase string

const (
	ThreadPhasePending     ThreadPhase = "Pending"
	ThreadPhaseActive      ThreadPhase = "Active"
	ThreadPhaseSleeping    ThreadPhase = "Sleeping"
	ThreadPhaseFailed      ThreadPhase = "Failed"
	ThreadPhaseTerminating ThreadPhase = "Terminating"
)

// ConditionTypes emitted by the operator. Stored on Status.Conditions
// using metav1.Condition. Callers should treat unknown conditions as
// non-fatal so the operator can add new conditions without breaking
// older consumers.
const (
	ConditionTypeReady             = "Ready"
	ConditionTypeWorkspaceProvisioned = "WorkspaceProvisioned"
	ConditionTypeDeploymentSynced  = "DeploymentSynced"
	ConditionTypeServiceSynced     = "ServiceSynced"
	ConditionTypeIngressSynced     = "IngressSynced"
	ConditionTypeUnmanagedConflict = "UnmanagedConflict"
)

// ThreadSpec is the user-facing contract of one thread workspace pod.
//
// Required fields: ThreadID, UserID, IngressHost, Image. Everything
// else has a documented default. The defaults intentionally match the
// existing remote/k8s/0[6-9]-thread-*.template.yaml manifests so a
// CR-driven thread is observably identical to a template-driven one.
type ThreadSpec struct {
	// ThreadID is the full UUID for this thread. Used as a label
	// (dd/threadId) on every child resource so existing selectors
	// (admin dashboard, idle reaper, gateway) keep working.
	ThreadID string `json:"threadId"`

	// ThreadIDShort is the short thread identifier used in resource
	// names. If empty, the operator derives it from ThreadID using
	// the same `<first8>-<last4>` shape used by the control plane.
	// +optional
	ThreadIDShort string `json:"threadIdShort,omitempty"`

	// UserID is the dd-user UUID that owns this thread. Stored as
	// the dd/userId label.
	UserID string `json:"userId"`

	// IngressHost is the public hostname under which the per-thread
	// path /dd-thread/<short>/* is served.
	IngressHost string `json:"ingressHost"`

	// Image is the dev-server container image. Mirrors
	// `image:` in 07-thread-deployment.template.yaml.
	Image string `json:"image"`

	// ImagePullPolicy defaults to IfNotPresent.
	// +optional
	ImagePullPolicy corev1.PullPolicy `json:"imagePullPolicy,omitempty"`

	// WorkspaceSize sets the per-thread PVC size. Defaults to 5Gi
	// (matches 06-thread-pvc.template.yaml).
	// +optional
	WorkspaceSize *resource.Quantity `json:"workspaceSize,omitempty"`

	// StorageClassName overrides the default StorageClass for the PVC.
	// Leave nil to use the cluster default (gp3 on EBS CSI on EC2).
	// +optional
	StorageClassName *string `json:"storageClassName,omitempty"`

	// ConfigMapName is envFrom'd into the dev-server container.
	// Defaults to "dd-agent-config".
	// +optional
	ConfigMapName string `json:"configMapName,omitempty"`

	// SecretName is envFrom'd into the dev-server container.
	// Defaults to "dd-agent-secrets".
	// +optional
	SecretName string `json:"secretName,omitempty"`

	// Resources lets a Thread override the container resource block.
	// Defaults match 07-thread-deployment.template.yaml.
	// +optional
	Resources *corev1.ResourceRequirements `json:"resources,omitempty"`

	// DesiredState selects sleep vs wake. Defaults to "Running" so a
	// freshly created CR comes up with replicas=1.
	// +optional
	DesiredState ThreadDesiredState `json:"desiredState,omitempty"`

	// IdleTimeoutSeconds enables operator-driven auto-sleep. If
	// LastActivityAt is set and (now - LastActivityAt) >
	// IdleTimeoutSeconds, the operator forces replicas=0 even when
	// DesiredState=Running. Set to 0 to disable.
	// +optional
	IdleTimeoutSeconds int64 `json:"idleTimeoutSeconds,omitempty"`

	// TTLSecondsAfterIdle, when set, deletes the entire Thread CR
	// (cascading to PVC, Deployment, Service, Ingress) once the
	// thread has been idle for that long. Leave nil to never auto-GC.
	// +optional
	TTLSecondsAfterIdle *int64 `json:"ttlSecondsAfterIdle,omitempty"`

	// LastActivityAt is set by external dispatchers (Vercel control
	// plane, dev-server) to mark fresh activity. The operator NEVER
	// writes to this field.
	// +optional
	LastActivityAt *metav1.Time `json:"lastActivityAt,omitempty"`
}

// ThreadStatus reflects the observed state of one Thread CR.
type ThreadStatus struct {
	// Phase is a coarse, human-readable rollup. Detailed signals
	// live on Conditions.
	// +optional
	Phase ThreadPhase `json:"phase,omitempty"`

	// ObservedGeneration tracks the last spec generation the
	// operator successfully reconciled. Lets clients tell when a
	// status update lags behind a spec edit.
	// +optional
	ObservedGeneration int64 `json:"observedGeneration,omitempty"`

	// PodIP / PodName / NodeName are only set when the thread Pod
	// is actually scheduled and ready.
	// +optional
	PodIP string `json:"podIP,omitempty"`
	// +optional
	PodName string `json:"podName,omitempty"`
	// +optional
	NodeName string `json:"nodeName,omitempty"`

	// ReplicasReady / ReplicasDesired mirror Deployment.Status.
	// +optional
	ReplicasReady int32 `json:"replicasReady"`
	// +optional
	ReplicasDesired int32 `json:"replicasDesired"`

	// LastReconcileTime helps operators monitor reconcile freshness.
	// +optional
	LastReconcileTime *metav1.Time `json:"lastReconcileTime,omitempty"`

	// Conditions surfaces detailed sync status per child resource.
	// +optional
	// +patchMergeKey=type
	// +patchStrategy=merge
	// +listType=map
	// +listMapKey=type
	Conditions []metav1.Condition `json:"conditions,omitempty" patchStrategy:"merge" patchMergeKey:"type"`
}

// +kubebuilder:object:root=true
// +kubebuilder:subresource:status
// +kubebuilder:resource:shortName=thrd,categories={dd,dd-remote-dev}
// +kubebuilder:printcolumn:name="Phase",type=string,JSONPath=`.status.phase`
// +kubebuilder:printcolumn:name="Desired",type=string,JSONPath=`.spec.desiredState`
// +kubebuilder:printcolumn:name="Ready",type=integer,JSONPath=`.status.replicasReady`
// +kubebuilder:printcolumn:name="ThreadID",type=string,JSONPath=`.spec.threadId`
// +kubebuilder:printcolumn:name="Age",type=date,JSONPath=`.metadata.creationTimestamp`

// Thread is one per-thread workspace pod, declared as a CRD instead
// of a hand-templated set of YAMLs.
type Thread struct {
	metav1.TypeMeta   `json:",inline"`
	metav1.ObjectMeta `json:"metadata,omitempty"`

	Spec   ThreadSpec   `json:"spec,omitempty"`
	Status ThreadStatus `json:"status,omitempty"`
}

// +kubebuilder:object:root=true

// ThreadList is the standard list wrapper for Thread.
type ThreadList struct {
	metav1.TypeMeta `json:",inline"`
	metav1.ListMeta `json:"metadata,omitempty"`
	Items           []Thread `json:"items"`
}

func init() {
	SchemeBuilder.Register(&Thread{}, &ThreadList{})
}
