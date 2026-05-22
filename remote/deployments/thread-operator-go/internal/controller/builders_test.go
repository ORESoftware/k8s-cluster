package controller

import (
	"strings"
	"testing"
	"time"

	corev1 "k8s.io/api/core/v1"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"

	threadv1 "github.com/ORESoftware/k8s-cluster/remote/deployments/thread-operator-go/api/v1alpha1"
)

// fixtureThread returns a Thread CR that matches the substitution
// shape produced by the existing template provisioning path, so the
// builders are tested against a realistic spec.
func fixtureThread() *threadv1.Thread {
	return &threadv1.Thread{
		ObjectMeta: metav1.ObjectMeta{
			Name:      "thread-001",
			Namespace: "dd-dev",
		},
		Spec: threadv1.ThreadSpec{
			ThreadID:      "deadbeef-cafe-4001-8001-feedfacefeed",
			ThreadIDShort: "deadbeefacefeed",
			UserID:        "00000000-0000-0000-0000-000000000001",
			IngressHost:   "agents.example.com",
			Image:         "REPLACE_ME_ECR_URI/dd-dev-server:latest",
			DesiredState:  threadv1.ThreadDesiredStateRunning,
		},
	}
}

func TestBuildPVCMatchesTemplateDefaults(t *testing.T) {
	thr := fixtureThread()
	pvc := BuildPVC(thr)

	if pvc.Name != "dd-thread-deadbeefacefeed" {
		t.Errorf("PVC name = %q, want dd-thread-deadbeefacefeed", pvc.Name)
	}
	if pvc.Namespace != "dd-dev" {
		t.Errorf("PVC namespace = %q, want dd-dev", pvc.Namespace)
	}
	if got := pvc.Spec.Resources.Requests[corev1.ResourceStorage]; got.String() != "5Gi" {
		t.Errorf("PVC storage = %q, want 5Gi", got.String())
	}
	if len(pvc.Spec.AccessModes) != 1 || pvc.Spec.AccessModes[0] != corev1.ReadWriteOnce {
		t.Errorf("PVC access modes = %v, want [ReadWriteOnce]", pvc.Spec.AccessModes)
	}
	if !HasManagedByLabel(pvc.Labels) {
		t.Errorf("PVC missing managed-by label: %v", pvc.Labels)
	}
}

func TestBuildDeploymentReplicasFromState(t *testing.T) {
	thr := fixtureThread()
	cases := []struct {
		name             string
		state            threadv1.ThreadDesiredState
		idleSeconds      int64
		lastActivity     time.Duration // ago
		hasLastActivity  bool
		expectedReplicas int32
	}{
		{name: "running default", state: threadv1.ThreadDesiredStateRunning, expectedReplicas: 1},
		{name: "sleeping", state: threadv1.ThreadDesiredStateSleeping, expectedReplicas: 0},
		{name: "running idle elapsed", state: threadv1.ThreadDesiredStateRunning, idleSeconds: 60, lastActivity: 5 * time.Minute, hasLastActivity: true, expectedReplicas: 0},
		{name: "running idle not elapsed", state: threadv1.ThreadDesiredStateRunning, idleSeconds: 600, lastActivity: 30 * time.Second, hasLastActivity: true, expectedReplicas: 1},
	}
	for _, c := range cases {
		t.Run(c.name, func(t *testing.T) {
			thr := *thr
			thr.Spec.DesiredState = c.state
			thr.Spec.IdleTimeoutSeconds = c.idleSeconds
			if c.hasLastActivity {
				ts := metav1.NewTime(time.Now().Add(-c.lastActivity))
				thr.Spec.LastActivityAt = &ts
			} else {
				thr.Spec.LastActivityAt = nil
			}
			got := decideReplicas(&thr)
			if got != c.expectedReplicas {
				t.Errorf("decideReplicas() = %d, want %d", got, c.expectedReplicas)
			}
		})
	}
}

func TestBuildDeploymentEnv(t *testing.T) {
	thr := fixtureThread()
	dep := BuildDeployment(thr, 1)
	if got := *dep.Spec.Replicas; got != 1 {
		t.Errorf("replicas = %d, want 1", got)
	}
	if got := dep.Spec.Strategy.Type; got != "Recreate" {
		t.Errorf("strategy = %q, want Recreate", got)
	}
	c := dep.Spec.Template.Spec.Containers[0]
	if c.Image != thr.Spec.Image {
		t.Errorf("image = %q, want %q", c.Image, thr.Spec.Image)
	}
	envByName := map[string]corev1.EnvVar{}
	for _, e := range c.Env {
		envByName[e.Name] = e
	}
	if envByName["REMOTE_DEV_THREAD_ID"].Value != thr.Spec.ThreadID {
		t.Errorf("REMOTE_DEV_THREAD_ID = %q, want %q", envByName["REMOTE_DEV_THREAD_ID"].Value, thr.Spec.ThreadID)
	}
	if envByName["USER_ID"].Value != thr.Spec.UserID {
		t.Errorf("USER_ID = %q, want %q", envByName["USER_ID"].Value, thr.Spec.UserID)
	}
}

func TestBuildIngressPathFormat(t *testing.T) {
	thr := fixtureThread()
	ing := BuildIngress(thr)
	if len(ing.Spec.Rules) != 1 {
		t.Fatalf("expected 1 rule, got %d", len(ing.Spec.Rules))
	}
	rule := ing.Spec.Rules[0]
	if rule.Host != thr.Spec.IngressHost {
		t.Errorf("host = %q, want %q", rule.Host, thr.Spec.IngressHost)
	}
	want := "/dd-thread/deadbeefacefeed(/.*)?"
	got := rule.HTTP.Paths[0].Path
	if got != want {
		t.Errorf("path = %q, want %q", got, want)
	}
}

func TestShortIDDeterministicWithoutOverride(t *testing.T) {
	thr := &threadv1.Thread{Spec: threadv1.ThreadSpec{ThreadID: "deadbeef-cafe-4001-8001-feedfacefeed"}}
	if got := ShortID(thr); got != "deadbeeffeed" {
		t.Errorf("ShortID = %q, want deadbeeffeed", got)
	}
	if !strings.HasPrefix(ChildName(thr), "dd-thread-") {
		t.Errorf("ChildName = %q, want dd-thread- prefix", ChildName(thr))
	}
}

func TestHasManagedByLabel(t *testing.T) {
	if HasManagedByLabel(nil) {
		t.Errorf("expected nil labels to NOT be managed-by")
	}
	if HasManagedByLabel(map[string]string{"app": "x"}) {
		t.Errorf("expected unrelated labels to NOT be managed-by")
	}
	if !HasManagedByLabel(map[string]string{"dd.dev/managed-by": "dd-thread-operator"}) {
		t.Errorf("expected managed-by label to be detected")
	}
}
