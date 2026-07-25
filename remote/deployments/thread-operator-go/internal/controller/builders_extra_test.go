package controller

// Additional table-driven unit tests for the PURE, deterministic
// resource-building logic (builders.go) plus the controller helper
// decideReplicas/evaluateTTL/derivePhase in thread_controller.go.
//
// These complement builders_test.go. Everything here runs with plain
// `go test ./...` — no controller-runtime envtest / apiserver required.
// The tests assert the ACTUAL current behavior (characterization) as
// well as the invariants documented in the package/godoc comments.

import (
	"encoding/hex"
	"strings"
	"testing"
	"time"

	appsv1 "k8s.io/api/apps/v1"
	corev1 "k8s.io/api/core/v1"
	networkingv1 "k8s.io/api/networking/v1"
	"k8s.io/apimachinery/pkg/api/resource"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"k8s.io/apimachinery/pkg/runtime"
	clientgoscheme "k8s.io/client-go/kubernetes/scheme"
	"sigs.k8s.io/controller-runtime/pkg/controller/controllerutil"

	threadv1 "github.com/ORESoftware/k8s-cluster/remote/deployments/thread-operator-go/api/v1alpha1"
)

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

// childObjects returns every child resource a Thread produces, typed as
// metav1.Object so cross-cutting metadata invariants (labels, name
// prefix, namespace, owner refs) can be asserted uniformly.
func childObjects(t *threadv1.Thread) map[string]metav1.Object {
	return map[string]metav1.Object{
		"PVC":        BuildPVC(t),
		"Deployment": BuildDeployment(t, 1),
		"Service":    BuildService(t),
		"Ingress":    BuildIngress(t),
	}
}

func findEnv(env []corev1.EnvVar, name string) (corev1.EnvVar, bool) {
	for _, e := range env {
		if e.Name == name {
			return e, true
		}
	}
	return corev1.EnvVar{}, false
}

func findVolume(vols []corev1.Volume, name string) (corev1.Volume, bool) {
	for _, v := range vols {
		if v.Name == name {
			return v, true
		}
	}
	return corev1.Volume{}, false
}

func findMount(mounts []corev1.VolumeMount, name string) (corev1.VolumeMount, bool) {
	for _, m := range mounts {
		if m.Name == name {
			return m, true
		}
	}
	return corev1.VolumeMount{}, false
}

// newTestScheme mirrors cmd/operator/main.go's scheme registration so
// SetControllerReference can resolve the Thread GVK exactly as the
// running operator does.
func newTestScheme(t *testing.T) *runtime.Scheme {
	t.Helper()
	s := runtime.NewScheme()
	if err := clientgoscheme.AddToScheme(s); err != nil {
		t.Fatalf("clientgoscheme.AddToScheme: %v", err)
	}
	if err := threadv1.AddToScheme(s); err != nil {
		t.Fatalf("threadv1.AddToScheme: %v", err)
	}
	return s
}

// ---------------------------------------------------------------------------
// ShortID / ChildName
// ---------------------------------------------------------------------------

func TestShortIDTable(t *testing.T) {
	cases := []struct {
		name  string
		short string
		full  string
		want  string
	}{
		// Explicit threadIdShort always wins, verbatim.
		{name: "override used verbatim", short: "deadbeefacefeed", full: "deadbeef-cafe-4001-8001-feedfacefeed", want: "deadbeefacefeed"},
		{name: "override trimmed of surrounding whitespace", short: "  padded-short  ", full: "deadbeef-cafe-4001-8001-feedfacefeed", want: "padded-short"},
		// Whitespace-only override falls through to the threadId branch.
		{name: "whitespace-only override falls back to threadId", short: "   ", full: "deadbeef-cafe-4001-8001-feedfacefeed", want: "deadbeeffeed"},
		// threadId >= 12: first8 + last4, dashes stripped.
		{name: "canonical uuid first8+last4", short: "", full: "deadbeef-cafe-4001-8001-feedfacefeed", want: "deadbeeffeed"},
		{name: "dashes inside first8 are stripped", short: "", full: "ab-cd-ef-gh-ijklmnop", want: "abcdefmnop"},
		{name: "exactly 12 chars returns whole id", short: "", full: "abcdefghijkl", want: "abcdefghijkl"}, // first8(abcdefgh)+last4(ijkl) == whole string
		// threadId < 12: sha256(full), first 6 bytes hex (12 chars).
		{name: "short threadId hashed", short: "", full: "short", want: "f9b0078b5df5"},
		{name: "two-char threadId hashed", short: "", full: "hi", want: "8f434346648f"},
		{name: "empty threadId hashed (never empty)", short: "", full: "", want: "e3b0c44298fc"},
	}
	for _, c := range cases {
		t.Run(c.name, func(t *testing.T) {
			thr := &threadv1.Thread{Spec: threadv1.ThreadSpec{ThreadID: c.full, ThreadIDShort: c.short}}
			if got := ShortID(thr); got != c.want {
				t.Errorf("ShortID(short=%q, full=%q) = %q, want %q", c.short, c.full, got, c.want)
			}
			// ChildName is always ShortID with the dd-thread- prefix.
			if got, want := ChildName(thr), "dd-thread-"+c.want; got != want {
				t.Errorf("ChildName = %q, want %q", got, want)
			}
		})
	}
}

func TestShortIDDeterministic(t *testing.T) {
	inputs := []threadv1.Thread{
		{Spec: threadv1.ThreadSpec{ThreadID: "deadbeef-cafe-4001-8001-feedfacefeed"}},
		{Spec: threadv1.ThreadSpec{ThreadID: "short"}},
		{Spec: threadv1.ThreadSpec{ThreadID: ""}},
		{Spec: threadv1.ThreadSpec{ThreadID: "x", ThreadIDShort: "explicit"}},
	}
	for _, in := range inputs {
		a := ShortID(&in)
		b := ShortID(&in)
		if a != b {
			t.Errorf("ShortID not deterministic for %+v: %q != %q", in.Spec, a, b)
		}
	}
}

func TestShortIDDistinctInputsDiffer(t *testing.T) {
	// Distinct threadIds should map to distinct short ids in both the
	// slice branch and the hash branch (no accidental collisions on
	// these fixtures).
	pairs := [][2]string{
		{"aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee", "11111111-2222-3333-4444-555555555555"},
		{"alpha", "bravo"}, // < 12 → hash branch
	}
	for _, p := range pairs {
		x := ShortID(&threadv1.Thread{Spec: threadv1.ThreadSpec{ThreadID: p[0]}})
		y := ShortID(&threadv1.Thread{Spec: threadv1.ThreadSpec{ThreadID: p[1]}})
		if x == y {
			t.Errorf("distinct threadIds %q/%q collided to %q", p[0], p[1], x)
		}
	}
}

func TestShortIDHashFallbackIsTwelveHexChars(t *testing.T) {
	// The <12-char branch encodes the first 6 bytes of a sha256, i.e.
	// exactly 12 lowercase hex characters, and is never empty.
	for _, full := range []string{"", "a", "short", "0123456789"} {
		got := ShortID(&threadv1.Thread{Spec: threadv1.ThreadSpec{ThreadID: full}})
		if len(got) != 12 {
			t.Errorf("ShortID(%q) length = %d, want 12 (%q)", full, len(got), got)
		}
		if _, err := hex.DecodeString(got); err != nil {
			t.Errorf("ShortID(%q) = %q is not valid hex: %v", full, got, err)
		}
		if got != strings.ToLower(got) {
			t.Errorf("ShortID(%q) = %q is not lowercase", full, got)
		}
	}
}

func TestChildNamePrefixedAndNonEmpty(t *testing.T) {
	// The documented invariant: ChildName never produces an empty name.
	for _, full := range []string{"", "hi", "short", "deadbeef-cafe-4001-8001-feedfacefeed"} {
		name := ChildName(&threadv1.Thread{Spec: threadv1.ThreadSpec{ThreadID: full}})
		if !strings.HasPrefix(name, "dd-thread-") {
			t.Errorf("ChildName(%q) = %q, missing dd-thread- prefix", full, name)
		}
		if name == "" {
			t.Errorf("ChildName(%q) is empty", full)
		}
		if len(name) <= len("dd-thread-") {
			t.Errorf("ChildName(%q) = %q has no suffix", full, name)
		}
	}
}

// TestShortIDAllDashesEdgeCase documents a latent edge in the >=12
// branch: an id composed entirely of removable dashes collapses to an
// EMPTY short id (first8+last4 are all '-', which ReplaceAll strips),
// so ChildName degrades to the bare prefix "dd-thread-". Real threadIds
// are UUIDs so this never fires in practice, but the behavior is pinned
// here so a future change is noticed. (Reported as an oddity, not fixed.)
func TestShortIDAllDashesEdgeCase(t *testing.T) {
	thr := &threadv1.Thread{Spec: threadv1.ThreadSpec{ThreadID: strings.Repeat("-", 12)}}
	if got := ShortID(thr); got != "" {
		t.Errorf("ShortID(all-dashes) = %q, want \"\" (current behavior)", got)
	}
	if got := ChildName(thr); got != "dd-thread-" {
		t.Errorf("ChildName(all-dashes) = %q, want \"dd-thread-\" (current behavior)", got)
	}
}

// ---------------------------------------------------------------------------
// label / naming invariants shared by every child
// ---------------------------------------------------------------------------

func TestEveryChildCarriesManagedByAndIdentityLabels(t *testing.T) {
	thr := fixtureThread()
	wantName := "dd-thread-deadbeefacefeed"
	for kind, obj := range childObjects(thr) {
		t.Run(kind, func(t *testing.T) {
			labels := obj.GetLabels()
			if !HasManagedByLabel(labels) {
				t.Errorf("%s missing managed-by label: %v", kind, labels)
			}
			checks := map[string]string{
				ManagedByLabel: ManagedByValue,
				PartOfLabel:    PartOfValue,
				ComponentLabel: ComponentValue,
				ThreadIDLabel:  thr.Spec.ThreadID,
				UserIDLabel:    thr.Spec.UserID,
			}
			for k, want := range checks {
				if got := labels[k]; got != want {
					t.Errorf("%s label %q = %q, want %q", kind, k, got, want)
				}
			}
			if obj.GetName() != wantName {
				t.Errorf("%s name = %q, want %q", kind, obj.GetName(), wantName)
			}
			if !strings.HasPrefix(obj.GetName(), "dd-thread-") {
				t.Errorf("%s name = %q, missing dd-thread- prefix", kind, obj.GetName())
			}
			if obj.GetNamespace() != thr.Namespace {
				t.Errorf("%s namespace = %q, want %q", kind, obj.GetNamespace(), thr.Namespace)
			}
			// Builders themselves never stamp owner refs; the reconciler
			// adds them via SetControllerReference.
			if refs := obj.GetOwnerReferences(); len(refs) != 0 {
				t.Errorf("%s builder set %d owner refs, want 0 (reconciler adds them)", kind, len(refs))
			}
		})
	}
}

func TestCommonLabelsExactSet(t *testing.T) {
	thr := fixtureThread()
	labels := CommonLabels(thr)
	want := map[string]string{
		PartOfLabel:    PartOfValue,
		ComponentLabel: ComponentValue,
		ManagedByLabel: ManagedByValue,
		ThreadIDLabel:  thr.Spec.ThreadID,
		UserIDLabel:    thr.Spec.UserID,
	}
	if len(labels) != len(want) {
		t.Errorf("CommonLabels has %d keys, want %d: %v", len(labels), len(want), labels)
	}
	for k, v := range want {
		if labels[k] != v {
			t.Errorf("CommonLabels[%q] = %q, want %q", k, labels[k], v)
		}
	}
}

func TestSelectorLabelsNarrowAndSubsetOfCommon(t *testing.T) {
	thr := fixtureThread()
	sel := SelectorLabels(thr)
	if len(sel) != 1 {
		t.Fatalf("SelectorLabels has %d keys, want 1 (narrow selector): %v", len(sel), sel)
	}
	if sel[ThreadIDLabel] != thr.Spec.ThreadID {
		t.Errorf("SelectorLabels[%q] = %q, want %q", ThreadIDLabel, sel[ThreadIDLabel], thr.Spec.ThreadID)
	}
	// Selector must be a subset of the common labels so the Deployment's
	// pod template (which carries common labels) is actually selected.
	common := CommonLabels(thr)
	for k, v := range sel {
		if common[k] != v {
			t.Errorf("selector label %q=%q not present in common labels", k, v)
		}
	}
}

func TestHasManagedByLabelValues(t *testing.T) {
	cases := []struct {
		name   string
		labels map[string]string
		want   bool
	}{
		{name: "nil", labels: nil, want: false},
		{name: "empty", labels: map[string]string{}, want: false},
		{name: "unrelated", labels: map[string]string{"app": "x"}, want: false},
		{name: "wrong value", labels: map[string]string{ManagedByLabel: "someone-else"}, want: false},
		{name: "correct", labels: map[string]string{ManagedByLabel: ManagedByValue}, want: true},
		{name: "correct among others", labels: map[string]string{"app": "x", ManagedByLabel: ManagedByValue}, want: true},
	}
	for _, c := range cases {
		t.Run(c.name, func(t *testing.T) {
			if got := HasManagedByLabel(c.labels); got != c.want {
				t.Errorf("HasManagedByLabel(%v) = %v, want %v", c.labels, got, c.want)
			}
		})
	}
}

// ---------------------------------------------------------------------------
// PVC
// ---------------------------------------------------------------------------

func TestBuildPVC(t *testing.T) {
	scName := "gp3"
	custom := resource.MustParse("20Gi")
	cases := []struct {
		name        string
		mutate      func(*threadv1.Thread)
		wantSize    string
		wantSCisNil bool
		wantSC      string
	}{
		{name: "default size, default storage class", mutate: func(*threadv1.Thread) {}, wantSize: "5Gi", wantSCisNil: true},
		{name: "custom workspace size", mutate: func(th *threadv1.Thread) { th.Spec.WorkspaceSize = &custom }, wantSize: "20Gi", wantSCisNil: true},
		{name: "explicit storage class", mutate: func(th *threadv1.Thread) { th.Spec.StorageClassName = &scName }, wantSize: "5Gi", wantSCisNil: false, wantSC: "gp3"},
	}
	for _, c := range cases {
		t.Run(c.name, func(t *testing.T) {
			thr := fixtureThread()
			c.mutate(thr)
			pvc := BuildPVC(thr)

			if got := pvc.Spec.Resources.Requests[corev1.ResourceStorage]; got.String() != c.wantSize {
				t.Errorf("storage = %q, want %q", got.String(), c.wantSize)
			}
			if len(pvc.Spec.AccessModes) != 1 || pvc.Spec.AccessModes[0] != corev1.ReadWriteOnce {
				t.Errorf("accessModes = %v, want [ReadWriteOnce]", pvc.Spec.AccessModes)
			}
			if c.wantSCisNil {
				if pvc.Spec.StorageClassName != nil {
					t.Errorf("storageClassName = %v, want nil", *pvc.Spec.StorageClassName)
				}
			} else {
				if pvc.Spec.StorageClassName == nil || *pvc.Spec.StorageClassName != c.wantSC {
					t.Errorf("storageClassName = %v, want %q", pvc.Spec.StorageClassName, c.wantSC)
				}
			}
		})
	}
}

// ---------------------------------------------------------------------------
// Deployment
// ---------------------------------------------------------------------------

func TestBuildDeploymentReplicasAndStrategy(t *testing.T) {
	thr := fixtureThread()
	for _, replicas := range []int32{0, 1, 3} {
		dep := BuildDeployment(thr, replicas)
		if dep.Spec.Replicas == nil || *dep.Spec.Replicas != replicas {
			t.Errorf("replicas = %v, want %d", dep.Spec.Replicas, replicas)
		}
	}
	dep := BuildDeployment(thr, 1)
	if dep.Spec.Strategy.Type != appsv1.RecreateDeploymentStrategyType {
		t.Errorf("strategy = %q, want Recreate", dep.Spec.Strategy.Type)
	}
	// Selector is the narrow (threadId-only) selector.
	if want := SelectorLabels(thr); !mapsEqual(dep.Spec.Selector.MatchLabels, want) {
		t.Errorf("selector = %v, want %v", dep.Spec.Selector.MatchLabels, want)
	}
	// Pod template carries the full common label set (so the selector
	// matches) — a superset of the selector.
	tmpl := dep.Spec.Template.Labels
	for k, v := range CommonLabels(thr) {
		if tmpl[k] != v {
			t.Errorf("pod template label %q = %q, want %q", k, tmpl[k], v)
		}
	}
	if tmpl[ThreadIDLabel] != thr.Spec.ThreadID {
		t.Errorf("pod template missing selector label %q", ThreadIDLabel)
	}
}

func TestBuildDeploymentPodSecurity(t *testing.T) {
	thr := fixtureThread()
	spec := BuildDeployment(thr, 1).Spec.Template.Spec

	if spec.ServiceAccountName != "dd-thread-pod" {
		t.Errorf("serviceAccountName = %q, want dd-thread-pod", spec.ServiceAccountName)
	}
	if spec.AutomountServiceAccountToken == nil || *spec.AutomountServiceAccountToken {
		t.Errorf("automountServiceAccountToken = %v, want false", spec.AutomountServiceAccountToken)
	}
	if spec.RestartPolicy != corev1.RestartPolicyAlways {
		t.Errorf("restartPolicy = %q, want Always", spec.RestartPolicy)
	}
	if spec.TerminationGracePeriodSeconds == nil || *spec.TerminationGracePeriodSeconds != 30 {
		t.Errorf("terminationGracePeriodSeconds = %v, want 30", spec.TerminationGracePeriodSeconds)
	}
	if spec.SecurityContext == nil {
		t.Fatal("pod securityContext is nil")
	}
	if spec.SecurityContext.RunAsNonRoot == nil || !*spec.SecurityContext.RunAsNonRoot {
		t.Errorf("runAsNonRoot = %v, want true", spec.SecurityContext.RunAsNonRoot)
	}
	for name, got := range map[string]*int64{
		"runAsUser":  spec.SecurityContext.RunAsUser,
		"runAsGroup": spec.SecurityContext.RunAsGroup,
		"fsGroup":    spec.SecurityContext.FSGroup,
	} {
		if got == nil || *got != 1000 {
			t.Errorf("pod securityContext.%s = %v, want 1000", name, got)
		}
	}
}

func TestBuildDeploymentContainer(t *testing.T) {
	thr := fixtureThread()
	c := BuildDeployment(thr, 1).Spec.Template.Spec.Containers[0]

	if c.Name != "dev-server" {
		t.Errorf("container name = %q, want dev-server", c.Name)
	}
	if c.Image != thr.Spec.Image {
		t.Errorf("image = %q, want %q", c.Image, thr.Spec.Image)
	}
	// Port: named http on 8080.
	if len(c.Ports) != 1 || c.Ports[0].Name != "http" || c.Ports[0].ContainerPort != 8080 {
		t.Errorf("ports = %+v, want single http/8080", c.Ports)
	}
	// Container security context.
	sc := c.SecurityContext
	if sc == nil {
		t.Fatal("container securityContext is nil")
	}
	if sc.AllowPrivilegeEscalation == nil || *sc.AllowPrivilegeEscalation {
		t.Errorf("allowPrivilegeEscalation = %v, want false", sc.AllowPrivilegeEscalation)
	}
	if sc.Capabilities == nil || len(sc.Capabilities.Drop) != 1 || sc.Capabilities.Drop[0] != "ALL" {
		t.Errorf("capabilities.drop = %v, want [ALL]", sc.Capabilities)
	}
	if sc.SeccompProfile == nil || sc.SeccompProfile.Type != corev1.SeccompProfileTypeRuntimeDefault {
		t.Errorf("seccompProfile = %v, want RuntimeDefault", sc.SeccompProfile)
	}

	// Value env vars.
	for name, want := range map[string]string{
		"REMOTE_DEV_THREAD_ID": thr.Spec.ThreadID,
		"USER_ID":              thr.Spec.UserID,
		"IDLE_TIMEOUT_MS":      "0",
	} {
		e, ok := findEnv(c.Env, name)
		if !ok {
			t.Errorf("env %q missing", name)
			continue
		}
		if e.Value != want {
			t.Errorf("env %q = %q, want %q", name, e.Value, want)
		}
	}
	// Downward-API env vars use fieldRef paths.
	for name, path := range map[string]string{
		"POD_NAME":  "metadata.name",
		"POD_IP":    "status.podIP",
		"NODE_NAME": "spec.nodeName",
	} {
		e, ok := findEnv(c.Env, name)
		if !ok || e.ValueFrom == nil || e.ValueFrom.FieldRef == nil {
			t.Errorf("env %q missing fieldRef", name)
			continue
		}
		if e.ValueFrom.FieldRef.FieldPath != path {
			t.Errorf("env %q fieldPath = %q, want %q", name, e.ValueFrom.FieldRef.FieldPath, path)
		}
	}

	// Volume mounts.
	if m, ok := findMount(c.VolumeMounts, "workspace"); !ok || m.MountPath != "/home/node/workspace" {
		t.Errorf("workspace mount = %+v, want /home/node/workspace", m)
	}
	if m, ok := findMount(c.VolumeMounts, "tmp-convos"); !ok || m.MountPath != "/tmp/convos" {
		t.Errorf("tmp-convos mount = %+v, want /tmp/convos", m)
	}

	// Probes: /healthz on the http port with the documented cadences.
	for name, p := range map[string]*corev1.Probe{
		"startup":   c.StartupProbe,
		"liveness":  c.LivenessProbe,
		"readiness": c.ReadinessProbe,
	} {
		if p == nil || p.HTTPGet == nil {
			t.Errorf("%s probe missing HTTPGet", name)
			continue
		}
		if p.HTTPGet.Path != "/healthz" || p.HTTPGet.Port.String() != "http" {
			t.Errorf("%s probe target = %s:%s, want /healthz:http", name, p.HTTPGet.Path, p.HTTPGet.Port.String())
		}
		if p.TimeoutSeconds != 5 {
			t.Errorf("%s probe timeout = %d, want 5", name, p.TimeoutSeconds)
		}
	}
	checkProbe := func(name string, p *corev1.Probe, period, failure int32) {
		if p.PeriodSeconds != period || p.FailureThreshold != failure {
			t.Errorf("%s probe period/failure = %d/%d, want %d/%d", name, p.PeriodSeconds, p.FailureThreshold, period, failure)
		}
	}
	checkProbe("startup", c.StartupProbe, 5, 24)
	checkProbe("liveness", c.LivenessProbe, 30, 3)
	checkProbe("readiness", c.ReadinessProbe, 10, 2)
}

func TestBuildDeploymentVolumes(t *testing.T) {
	thr := fixtureThread()
	vols := BuildDeployment(thr, 1).Spec.Template.Spec.Volumes

	ws, ok := findVolume(vols, "workspace")
	if !ok || ws.PersistentVolumeClaim == nil {
		t.Fatalf("workspace volume missing PVC source: %+v", ws)
	}
	if ws.PersistentVolumeClaim.ClaimName != ChildName(thr) {
		t.Errorf("workspace claimName = %q, want %q", ws.PersistentVolumeClaim.ClaimName, ChildName(thr))
	}
	tmp, ok := findVolume(vols, "tmp-convos")
	if !ok || tmp.EmptyDir == nil {
		t.Fatalf("tmp-convos volume missing EmptyDir source: %+v", tmp)
	}
	if tmp.EmptyDir.SizeLimit == nil || tmp.EmptyDir.SizeLimit.String() != "256Mi" {
		t.Errorf("tmp-convos sizeLimit = %v, want 256Mi", tmp.EmptyDir.SizeLimit)
	}
}

func TestBuildDeploymentDefaultsAndOverrides(t *testing.T) {
	t.Run("defaults", func(t *testing.T) {
		thr := fixtureThread() // no configMap/secret/pullPolicy/resources set
		c := BuildDeployment(thr, 1).Spec.Template.Spec.Containers[0]
		if c.ImagePullPolicy != corev1.PullIfNotPresent {
			t.Errorf("imagePullPolicy = %q, want IfNotPresent", c.ImagePullPolicy)
		}
		gotCM, gotSecret := envFromSources(c.EnvFrom)
		if gotCM != "dd-agent-config" {
			t.Errorf("configMap envFrom = %q, want dd-agent-config", gotCM)
		}
		if gotSecret != "dd-agent-secrets" {
			t.Errorf("secret envFrom = %q, want dd-agent-secrets", gotSecret)
		}
		// Default resource requests/limits.
		wantReq := map[corev1.ResourceName]string{corev1.ResourceCPU: "1m", corev1.ResourceMemory: "512Mi"}
		wantLim := map[corev1.ResourceName]string{corev1.ResourceCPU: "2", corev1.ResourceMemory: "4Gi"}
		for k, want := range wantReq {
			if got := c.Resources.Requests[k]; got.String() != want {
				t.Errorf("request %s = %q, want %q", k, got.String(), want)
			}
		}
		for k, want := range wantLim {
			if got := c.Resources.Limits[k]; got.String() != want {
				t.Errorf("limit %s = %q, want %q", k, got.String(), want)
			}
		}
	})

	t.Run("overrides", func(t *testing.T) {
		thr := fixtureThread()
		thr.Spec.ConfigMapName = "custom-config"
		thr.Spec.SecretName = "custom-secret"
		thr.Spec.ImagePullPolicy = corev1.PullAlways
		thr.Spec.Resources = &corev1.ResourceRequirements{
			Requests: corev1.ResourceList{corev1.ResourceCPU: resource.MustParse("250m")},
			Limits:   corev1.ResourceList{corev1.ResourceMemory: resource.MustParse("8Gi")},
		}
		c := BuildDeployment(thr, 1).Spec.Template.Spec.Containers[0]
		if c.ImagePullPolicy != corev1.PullAlways {
			t.Errorf("imagePullPolicy = %q, want Always", c.ImagePullPolicy)
		}
		gotCM, gotSecret := envFromSources(c.EnvFrom)
		if gotCM != "custom-config" || gotSecret != "custom-secret" {
			t.Errorf("envFrom = (%q,%q), want (custom-config,custom-secret)", gotCM, gotSecret)
		}
		if got := c.Resources.Requests[corev1.ResourceCPU]; got.String() != "250m" {
			t.Errorf("overridden cpu request = %q, want 250m", got.String())
		}
		if got := c.Resources.Limits[corev1.ResourceMemory]; got.String() != "8Gi" {
			t.Errorf("overridden mem limit = %q, want 8Gi", got.String())
		}
	})
}

// TestBuildDeploymentIdleEnvIsConstantZero characterizes that the
// container's IDLE_TIMEOUT_MS is hard-coded to "0" regardless of
// spec.IdleTimeoutSeconds; auto-sleep is enforced by the operator
// (decideReplicas), not by an in-container idle timer.
func TestBuildDeploymentIdleEnvIsConstantZero(t *testing.T) {
	thr := fixtureThread()
	thr.Spec.IdleTimeoutSeconds = 600
	c := BuildDeployment(thr, 1).Spec.Template.Spec.Containers[0]
	e, ok := findEnv(c.Env, "IDLE_TIMEOUT_MS")
	if !ok || e.Value != "0" {
		t.Errorf("IDLE_TIMEOUT_MS = %q (present=%v), want \"0\" even with IdleTimeoutSeconds=600", e.Value, ok)
	}
}

// ---------------------------------------------------------------------------
// Service
// ---------------------------------------------------------------------------

func TestBuildService(t *testing.T) {
	thr := fixtureThread()
	svc := BuildService(thr)

	if svc.Spec.Type != corev1.ServiceTypeClusterIP {
		t.Errorf("type = %q, want ClusterIP", svc.Spec.Type)
	}
	if !mapsEqual(svc.Spec.Selector, SelectorLabels(thr)) {
		t.Errorf("selector = %v, want %v", svc.Spec.Selector, SelectorLabels(thr))
	}
	if len(svc.Spec.Ports) != 1 {
		t.Fatalf("ports = %d, want 1", len(svc.Spec.Ports))
	}
	p := svc.Spec.Ports[0]
	if p.Name != "http" || p.Port != 8080 || p.TargetPort.String() != "http" {
		t.Errorf("port = %+v, want http/8080 -> http", p)
	}
}

// ---------------------------------------------------------------------------
// Ingress
// ---------------------------------------------------------------------------

func TestBuildIngress(t *testing.T) {
	thr := fixtureThread()
	ing := BuildIngress(thr)

	if ing.Spec.IngressClassName == nil || *ing.Spec.IngressClassName != "nginx" {
		t.Errorf("ingressClassName = %v, want nginx", ing.Spec.IngressClassName)
	}
	// TLS: shared dd-threads-tls secret scoped to the ingress host.
	if len(ing.Spec.TLS) != 1 || ing.Spec.TLS[0].SecretName != "dd-threads-tls" {
		t.Fatalf("TLS = %+v, want single dd-threads-tls", ing.Spec.TLS)
	}
	if len(ing.Spec.TLS[0].Hosts) != 1 || ing.Spec.TLS[0].Hosts[0] != thr.Spec.IngressHost {
		t.Errorf("TLS hosts = %v, want [%q]", ing.Spec.TLS[0].Hosts, thr.Spec.IngressHost)
	}
	// Annotations.
	wantAnn := map[string]string{
		"nginx.ingress.kubernetes.io/use-regex":          "true",
		"nginx.ingress.kubernetes.io/rewrite-target":     "/$1",
		"nginx.ingress.kubernetes.io/proxy-read-timeout": "900",
		"nginx.ingress.kubernetes.io/proxy-send-timeout": "900",
		"nginx.ingress.kubernetes.io/proxy-buffering":    "off",
	}
	for k, v := range wantAnn {
		if ing.Annotations[k] != v {
			t.Errorf("annotation %q = %q, want %q", k, ing.Annotations[k], v)
		}
	}
	// Rule + path + backend.
	if len(ing.Spec.Rules) != 1 {
		t.Fatalf("rules = %d, want 1", len(ing.Spec.Rules))
	}
	rule := ing.Spec.Rules[0]
	if rule.Host != thr.Spec.IngressHost {
		t.Errorf("rule host = %q, want %q", rule.Host, thr.Spec.IngressHost)
	}
	path := rule.HTTP.Paths[0]
	if path.Path != "/dd-thread/deadbeefacefeed(/.*)?" {
		t.Errorf("path = %q, want /dd-thread/deadbeefacefeed(/.*)?", path.Path)
	}
	if path.PathType == nil || *path.PathType != networkingv1.PathTypeImplementationSpecific {
		t.Errorf("pathType = %v, want ImplementationSpecific", path.PathType)
	}
	if path.Backend.Service == nil || path.Backend.Service.Name != ChildName(thr) {
		t.Errorf("backend service = %+v, want %q", path.Backend.Service, ChildName(thr))
	}
	if path.Backend.Service.Port.Number != 8080 {
		t.Errorf("backend port = %d, want 8080", path.Backend.Service.Port.Number)
	}
}

// TestBuildIngressPathUsesShortIDNotFullThreadID confirms the ingress
// path is keyed on the short id (ShortID), derived from threadId when
// no threadIdShort is supplied — not the full threadId.
func TestBuildIngressPathUsesShortIDNotFullThreadID(t *testing.T) {
	thr := fixtureThread()
	thr.Spec.ThreadIDShort = "" // force derivation from threadId
	ing := BuildIngress(thr)
	want := "/dd-thread/" + ShortID(thr) + "(/.*)?"
	if got := ing.Spec.Rules[0].HTTP.Paths[0].Path; got != want {
		t.Errorf("path = %q, want %q", got, want)
	}
	if strings.Contains(ing.Spec.Rules[0].HTTP.Paths[0].Path, thr.Spec.ThreadID) {
		t.Errorf("path %q must not embed the full threadId", ing.Spec.Rules[0].HTTP.Paths[0].Path)
	}
}

// ---------------------------------------------------------------------------
// naming consistency across children
// ---------------------------------------------------------------------------

func TestChildNamesAreInternallyConsistent(t *testing.T) {
	thr := fixtureThread()
	want := ChildName(thr)

	dep := BuildDeployment(thr, 1)
	svc := BuildService(thr)
	ing := BuildIngress(thr)
	pvc := BuildPVC(thr)

	if pvc.Name != want || dep.Name != want || svc.Name != want || ing.Name != want {
		t.Errorf("child names diverge: pvc=%q dep=%q svc=%q ing=%q want=%q", pvc.Name, dep.Name, svc.Name, ing.Name, want)
	}
	// The Deployment's PVC volume must reference the PVC by the same name.
	ws, _ := findVolume(dep.Spec.Template.Spec.Volumes, "workspace")
	if ws.PersistentVolumeClaim == nil || ws.PersistentVolumeClaim.ClaimName != pvc.Name {
		t.Errorf("deployment workspace claim = %v, want %q", ws.PersistentVolumeClaim, pvc.Name)
	}
	// The Ingress backend must reference the Service by the same name.
	if ing.Spec.Rules[0].HTTP.Paths[0].Backend.Service.Name != svc.Name {
		t.Errorf("ingress backend = %q, want %q", ing.Spec.Rules[0].HTTP.Paths[0].Backend.Service.Name, svc.Name)
	}
}

// ---------------------------------------------------------------------------
// OwnerReferences (as wired by the reconciler via SetControllerReference)
// ---------------------------------------------------------------------------

func TestSetControllerReferenceOnBuiltChildren(t *testing.T) {
	scheme := newTestScheme(t)
	thr := fixtureThread()
	thr.UID = "11111111-2222-3333-4444-555555555555"

	children := map[string]interface {
		metav1.Object
		runtime.Object
	}{
		"PVC":        BuildPVC(thr),
		"Deployment": BuildDeployment(thr, 1),
		"Service":    BuildService(thr),
		"Ingress":    BuildIngress(thr),
	}
	for kind, child := range children {
		t.Run(kind, func(t *testing.T) {
			if err := controllerutil.SetControllerReference(thr, child, scheme); err != nil {
				t.Fatalf("SetControllerReference: %v", err)
			}
			refs := child.GetOwnerReferences()
			if len(refs) != 1 {
				t.Fatalf("owner refs = %d, want 1", len(refs))
			}
			ref := refs[0]
			if ref.APIVersion != "dd.dev/v1alpha1" {
				t.Errorf("owner apiVersion = %q, want dd.dev/v1alpha1", ref.APIVersion)
			}
			if ref.Kind != "Thread" {
				t.Errorf("owner kind = %q, want Thread", ref.Kind)
			}
			if ref.Name != thr.Name {
				t.Errorf("owner name = %q, want %q", ref.Name, thr.Name)
			}
			if ref.UID != thr.UID {
				t.Errorf("owner uid = %q, want %q", ref.UID, thr.UID)
			}
			if ref.Controller == nil || !*ref.Controller {
				t.Errorf("owner controller = %v, want true", ref.Controller)
			}
			if ref.BlockOwnerDeletion == nil || !*ref.BlockOwnerDeletion {
				t.Errorf("owner blockOwnerDeletion = %v, want true", ref.BlockOwnerDeletion)
			}
		})
	}
}

// ---------------------------------------------------------------------------
// controller helpers: evaluateTTL / derivePhase
// (decideReplicas is already covered by builders_test.go)
// ---------------------------------------------------------------------------

func TestEvaluateTTL(t *testing.T) {
	ttl := int64(60)
	cases := []struct {
		name         string
		ttl          *int64
		lastActivity *time.Duration // ago; nil => LastActivityAt unset
		wantDelete   bool
		wantRequeue  bool // whether a positive requeue duration is returned
	}{
		{name: "ttl unset", ttl: nil, lastActivity: durPtr(2 * time.Hour), wantDelete: false, wantRequeue: false},
		{name: "no last activity", ttl: &ttl, lastActivity: nil, wantDelete: false, wantRequeue: false},
		{name: "ttl elapsed => delete", ttl: &ttl, lastActivity: durPtr(5 * time.Minute), wantDelete: true, wantRequeue: false},
		{name: "ttl pending => requeue", ttl: &ttl, lastActivity: durPtr(10 * time.Second), wantDelete: false, wantRequeue: true},
	}
	for _, c := range cases {
		t.Run(c.name, func(t *testing.T) {
			thr := fixtureThread()
			thr.Spec.TTLSecondsAfterIdle = c.ttl
			if c.lastActivity != nil {
				ts := metav1.NewTime(time.Now().Add(-*c.lastActivity))
				thr.Spec.LastActivityAt = &ts
			}
			del, after := evaluateTTL(thr)
			if del != c.wantDelete {
				t.Errorf("delete = %v, want %v", del, c.wantDelete)
			}
			if (after > 0) != c.wantRequeue {
				t.Errorf("requeueAfter = %v, wantPositive = %v", after, c.wantRequeue)
			}
			// evaluateTTL never returns a requeue longer than defaultRequeue.
			if after > defaultRequeue {
				t.Errorf("requeueAfter = %v exceeds defaultRequeue %v", after, defaultRequeue)
			}
		})
	}
}

// TestEvaluateTTLBoundsLongRequeue characterizes the clamp: a far-future
// deadline still requeues within defaultRequeue so periodic idle checks
// are never skipped.
func TestEvaluateTTLBoundsLongRequeue(t *testing.T) {
	ttl := int64(3600) // 1h TTL
	thr := fixtureThread()
	thr.Spec.TTLSecondsAfterIdle = &ttl
	ts := metav1.NewTime(time.Now()) // just active => deadline ~1h out
	thr.Spec.LastActivityAt = &ts
	del, after := evaluateTTL(thr)
	if del {
		t.Errorf("delete = true, want false (TTL far from elapsing)")
	}
	if after != defaultRequeue {
		t.Errorf("requeueAfter = %v, want clamp to defaultRequeue %v", after, defaultRequeue)
	}
}

func TestDerivePhase(t *testing.T) {
	dep := func(ready, unavailable int32) *appsv1.Deployment {
		d := &appsv1.Deployment{}
		d.Status.ReadyReplicas = ready
		d.Status.UnavailableReplicas = unavailable
		return d
	}
	cases := []struct {
		name            string
		dep             *appsv1.Deployment
		desiredReplicas int32
		deleting        bool
		want            threadv1.ThreadPhase
	}{
		{name: "deleting", dep: dep(1, 0), desiredReplicas: 1, deleting: true, want: threadv1.ThreadPhaseTerminating},
		{name: "no deployment yet", dep: nil, desiredReplicas: 1, want: threadv1.ThreadPhasePending},
		{name: "sleeping (desired 0)", dep: dep(0, 0), desiredReplicas: 0, want: threadv1.ThreadPhaseSleeping},
		{name: "failed (unavailable, none ready)", dep: dep(0, 1), desiredReplicas: 1, want: threadv1.ThreadPhaseFailed},
		{name: "active (ready >= desired)", dep: dep(1, 0), desiredReplicas: 1, want: threadv1.ThreadPhaseActive},
		{name: "pending (ready < desired)", dep: dep(0, 0), desiredReplicas: 1, want: threadv1.ThreadPhasePending},
	}
	for _, c := range cases {
		t.Run(c.name, func(t *testing.T) {
			thr := fixtureThread()
			if c.deleting {
				now := metav1.NewTime(time.Now())
				thr.DeletionTimestamp = &now
			}
			if got := derivePhase(thr, c.dep, c.desiredReplicas); got != c.want {
				t.Errorf("derivePhase = %q, want %q", got, c.want)
			}
		})
	}
}

// ---------------------------------------------------------------------------
// small local helpers
// ---------------------------------------------------------------------------

func durPtr(d time.Duration) *time.Duration { return &d }

func mapsEqual(a, b map[string]string) bool {
	if len(a) != len(b) {
		return false
	}
	for k, v := range a {
		if b[k] != v {
			return false
		}
	}
	return true
}

// envFromSources extracts the (configMapName, secretName) referenced by
// a container's envFrom list.
func envFromSources(sources []corev1.EnvFromSource) (configMap, secret string) {
	for _, s := range sources {
		if s.ConfigMapRef != nil {
			configMap = s.ConfigMapRef.Name
		}
		if s.SecretRef != nil {
			secret = s.SecretRef.Name
		}
	}
	return configMap, secret
}
