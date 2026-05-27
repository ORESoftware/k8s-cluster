// Builders for Thread child resources. The output of these helpers
// is intentionally byte-for-byte aligned with the templates in
// remote/k8s/0[6-9]-thread-*.template.yaml so a CR-driven thread is
// observably identical to a template-driven thread (modulo
// OwnerReferences and the dd.dev/managed-by label).
package controller

import (
	"crypto/sha256"
	"encoding/hex"
	"fmt"
	"strings"

	appsv1 "k8s.io/api/apps/v1"
	corev1 "k8s.io/api/core/v1"
	networkingv1 "k8s.io/api/networking/v1"
	"k8s.io/apimachinery/pkg/api/resource"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"k8s.io/apimachinery/pkg/util/intstr"

	threadv1 "github.com/ORESoftware/k8s-cluster/remote/deployments/thread-operator-go/api/v1alpha1"
)

// ManagedByLabel marks every resource the operator owns. The
// operator REFUSES to mutate a child resource that has the same
// name as one of its expected children but is missing this label,
// so an existing template-provisioned thread can never be silently
// adopted.
const (
	ManagedByLabel       = "dd.dev/managed-by"
	ManagedByValue       = "dd-thread-operator"
	ThreadIDLabel        = "dd/threadId"
	UserIDLabel          = "dd/userId"
	PartOfLabel          = "app.kubernetes.io/part-of"
	PartOfValue          = "dd-remote-dev"
	ComponentLabel       = "app.kubernetes.io/component"
	ComponentValue       = "thread-pod"
	ThreadShortHashLabel = "dd.dev/thread-short-hash"

	defaultConfigMapName = "dd-agent-config"
	defaultSecretName    = "dd-agent-secrets"
	defaultPVCSize       = "5Gi"
	resourceNamePrefix   = "dd-thread-"
)

// ChildName is the deterministic name used for every child resource
// of one Thread. Falls back to a hash-derived short ID when
// spec.threadIdShort is empty so we never produce empty names.
func ChildName(t *threadv1.Thread) string {
	return resourceNamePrefix + ShortID(t)
}

// ShortID picks the user-supplied threadIdShort when present,
// otherwise derives a stable 12-char id from threadId.
func ShortID(t *threadv1.Thread) string {
	if s := strings.TrimSpace(t.Spec.ThreadIDShort); s != "" {
		return s
	}
	full := t.Spec.ThreadID
	if len(full) >= 12 {
		// Match the control-plane convention: first 8 + last 4.
		return strings.ReplaceAll(full[:8]+full[len(full)-4:], "-", "")
	}
	sum := sha256.Sum256([]byte(full))
	return hex.EncodeToString(sum[:6])
}

// CommonLabels are the labels every Thread child carries.
func CommonLabels(t *threadv1.Thread) map[string]string {
	return map[string]string{
		PartOfLabel:    PartOfValue,
		ComponentLabel: ComponentValue,
		ManagedByLabel: ManagedByValue,
		ThreadIDLabel:  t.Spec.ThreadID,
		UserIDLabel:    t.Spec.UserID,
	}
}

// SelectorLabels are the labels used to select the per-thread Pod.
// Kept intentionally narrow (matches 07-thread-deployment.template.yaml)
// so existing tooling that selects on dd/threadId keeps working.
func SelectorLabels(t *threadv1.Thread) map[string]string {
	return map[string]string{
		ThreadIDLabel: t.Spec.ThreadID,
	}
}

// HasManagedByLabel reports whether the given object's labels mark
// it as operator-owned. The reconciler uses this to refuse to
// mutate template-provisioned children even when names collide.
func HasManagedByLabel(labels map[string]string) bool {
	return labels[ManagedByLabel] == ManagedByValue
}

// BuildPVC returns the desired PVC for one Thread. Storage class
// defaults to the cluster default; spec.storageClassName overrides.
func BuildPVC(t *threadv1.Thread) *corev1.PersistentVolumeClaim {
	size := resource.MustParse(defaultPVCSize)
	if t.Spec.WorkspaceSize != nil {
		size = *t.Spec.WorkspaceSize
	}
	pvc := &corev1.PersistentVolumeClaim{
		ObjectMeta: metav1.ObjectMeta{
			Name:      ChildName(t),
			Namespace: t.Namespace,
			Labels:    CommonLabels(t),
		},
		Spec: corev1.PersistentVolumeClaimSpec{
			AccessModes: []corev1.PersistentVolumeAccessMode{corev1.ReadWriteOnce},
			Resources: corev1.VolumeResourceRequirements{
				Requests: corev1.ResourceList{
					corev1.ResourceStorage: size,
				},
			},
		},
	}
	if t.Spec.StorageClassName != nil {
		pvc.Spec.StorageClassName = t.Spec.StorageClassName
	}
	return pvc
}

// BuildDeployment returns the desired Deployment for one Thread.
// `desiredReplicas` is computed by the reconciler from
// spec.desiredState plus idle-timeout policy.
func BuildDeployment(t *threadv1.Thread, desiredReplicas int32) *appsv1.Deployment {
	configMap := t.Spec.ConfigMapName
	if configMap == "" {
		configMap = defaultConfigMapName
	}
	secret := t.Spec.SecretName
	if secret == "" {
		secret = defaultSecretName
	}
	pullPolicy := t.Spec.ImagePullPolicy
	if pullPolicy == "" {
		pullPolicy = corev1.PullIfNotPresent
	}

	resources := corev1.ResourceRequirements{
		Requests: corev1.ResourceList{
			corev1.ResourceCPU:    resource.MustParse("1m"),
			corev1.ResourceMemory: resource.MustParse("512Mi"),
		},
		Limits: corev1.ResourceList{
			corev1.ResourceCPU:    resource.MustParse("2"),
			corev1.ResourceMemory: resource.MustParse("4Gi"),
		},
	}
	if t.Spec.Resources != nil {
		resources = *t.Spec.Resources
	}

	httpProbe := func(periodSeconds, failureThreshold int32) *corev1.Probe {
		return &corev1.Probe{
			ProbeHandler: corev1.ProbeHandler{
				HTTPGet: &corev1.HTTPGetAction{
					Path: "/healthz",
					Port: intstr.FromString("http"),
				},
			},
			PeriodSeconds:    periodSeconds,
			FailureThreshold: failureThreshold,
			TimeoutSeconds:   5,
		}
	}

	labels := CommonLabels(t)
	selector := SelectorLabels(t)
	runAsNonRoot := true
	allowPrivilegeEscalation := false

	dep := &appsv1.Deployment{
		ObjectMeta: metav1.ObjectMeta{
			Name:      ChildName(t),
			Namespace: t.Namespace,
			Labels:    labels,
		},
		Spec: appsv1.DeploymentSpec{
			Replicas: &desiredReplicas,
			Strategy: appsv1.DeploymentStrategy{Type: appsv1.RecreateDeploymentStrategyType},
			Selector: &metav1.LabelSelector{MatchLabels: selector},
			Template: corev1.PodTemplateSpec{
				ObjectMeta: metav1.ObjectMeta{Labels: mergeLabels(selector, labels)},
				Spec: corev1.PodSpec{
					ServiceAccountName:           "dd-thread-pod",
					AutomountServiceAccountToken: ptrBool(false),
					RestartPolicy:                corev1.RestartPolicyAlways,
					TerminationGracePeriodSeconds: ptrInt64(30),
					SecurityContext: &corev1.PodSecurityContext{
						RunAsNonRoot: &runAsNonRoot,
						RunAsUser:    ptrInt64(1000),
						RunAsGroup:   ptrInt64(1000),
						FSGroup:      ptrInt64(1000),
					},
					Containers: []corev1.Container{{
						Name:            "dev-server",
						Image:           t.Spec.Image,
						ImagePullPolicy: pullPolicy,
						SecurityContext: &corev1.SecurityContext{
							AllowPrivilegeEscalation: &allowPrivilegeEscalation,
							Capabilities: &corev1.Capabilities{
								Drop: []corev1.Capability{"ALL"},
							},
							SeccompProfile: &corev1.SeccompProfile{
								Type: corev1.SeccompProfileTypeRuntimeDefault,
							},
						},
						Ports: []corev1.ContainerPort{{
							Name:          "http",
							ContainerPort: 8080,
						}},
						EnvFrom: []corev1.EnvFromSource{
							{ConfigMapRef: &corev1.ConfigMapEnvSource{LocalObjectReference: corev1.LocalObjectReference{Name: configMap}}},
							{SecretRef: &corev1.SecretEnvSource{LocalObjectReference: corev1.LocalObjectReference{Name: secret}}},
						},
						Env: []corev1.EnvVar{
							{Name: "REMOTE_DEV_THREAD_ID", Value: t.Spec.ThreadID},
							{Name: "USER_ID", Value: t.Spec.UserID},
							{Name: "IDLE_TIMEOUT_MS", Value: "0"},
							{Name: "POD_NAME", ValueFrom: &corev1.EnvVarSource{FieldRef: &corev1.ObjectFieldSelector{FieldPath: "metadata.name"}}},
							{Name: "POD_IP", ValueFrom: &corev1.EnvVarSource{FieldRef: &corev1.ObjectFieldSelector{FieldPath: "status.podIP"}}},
							{Name: "NODE_NAME", ValueFrom: &corev1.EnvVarSource{FieldRef: &corev1.ObjectFieldSelector{FieldPath: "spec.nodeName"}}},
						},
						VolumeMounts: []corev1.VolumeMount{
							{Name: "workspace", MountPath: "/home/node/workspace"},
							{Name: "tmp-convos", MountPath: "/tmp/convos"},
						},
						Resources:      resources,
						StartupProbe:   httpProbe(5, 24),
						LivenessProbe:  httpProbe(30, 3),
						ReadinessProbe: httpProbe(10, 2),
					}},
					Volumes: []corev1.Volume{
						{
							Name: "workspace",
							VolumeSource: corev1.VolumeSource{
								PersistentVolumeClaim: &corev1.PersistentVolumeClaimVolumeSource{
									ClaimName: ChildName(t),
								},
							},
						},
						{
							Name: "tmp-convos",
							VolumeSource: corev1.VolumeSource{
								EmptyDir: &corev1.EmptyDirVolumeSource{
									SizeLimit: ptrQuantity("256Mi"),
								},
							},
						},
					},
				},
			},
		},
	}
	return dep
}

// BuildService returns the desired ClusterIP Service for one Thread.
func BuildService(t *threadv1.Thread) *corev1.Service {
	return &corev1.Service{
		ObjectMeta: metav1.ObjectMeta{
			Name:      ChildName(t),
			Namespace: t.Namespace,
			Labels:    CommonLabels(t),
		},
		Spec: corev1.ServiceSpec{
			Type:     corev1.ServiceTypeClusterIP,
			Selector: SelectorLabels(t),
			Ports: []corev1.ServicePort{{
				Name:       "http",
				Port:       8080,
				TargetPort: intstr.FromString("http"),
			}},
		},
	}
}

// BuildIngress returns the desired Ingress for one Thread. Mirrors
// 09-thread-ingress.template.yaml: shared host, path-based per-thread,
// shared dd-threads-tls secret, SSE-friendly proxy timings.
func BuildIngress(t *threadv1.Thread) *networkingv1.Ingress {
	pathType := networkingv1.PathTypeImplementationSpecific
	className := "nginx"
	short := ShortID(t)
	return &networkingv1.Ingress{
		ObjectMeta: metav1.ObjectMeta{
			Name:      ChildName(t),
			Namespace: t.Namespace,
			Labels:    CommonLabels(t),
			Annotations: map[string]string{
				"nginx.ingress.kubernetes.io/use-regex":          "true",
				"nginx.ingress.kubernetes.io/rewrite-target":     "/$1",
				"nginx.ingress.kubernetes.io/proxy-read-timeout": "900",
				"nginx.ingress.kubernetes.io/proxy-send-timeout": "900",
				"nginx.ingress.kubernetes.io/proxy-buffering":    "off",
			},
		},
		Spec: networkingv1.IngressSpec{
			IngressClassName: &className,
			TLS: []networkingv1.IngressTLS{{
				Hosts:      []string{t.Spec.IngressHost},
				SecretName: "dd-threads-tls",
			}},
			Rules: []networkingv1.IngressRule{{
				Host: t.Spec.IngressHost,
				IngressRuleValue: networkingv1.IngressRuleValue{
					HTTP: &networkingv1.HTTPIngressRuleValue{
						Paths: []networkingv1.HTTPIngressPath{{
							Path:     fmt.Sprintf("/dd-thread/%s(/.*)?", short),
							PathType: &pathType,
							Backend: networkingv1.IngressBackend{
								Service: &networkingv1.IngressServiceBackend{
									Name: ChildName(t),
									Port: networkingv1.ServiceBackendPort{Number: 8080},
								},
							},
						}},
					},
				},
			}},
		},
	}
}

func mergeLabels(a, b map[string]string) map[string]string {
	out := make(map[string]string, len(a)+len(b))
	for k, v := range b {
		out[k] = v
	}
	for k, v := range a {
		out[k] = v
	}
	return out
}

func ptrBool(b bool) *bool       { return &b }
func ptrInt64(v int64) *int64    { return &v }
func ptrQuantity(s string) *resource.Quantity {
	q := resource.MustParse(s)
	return &q
}
