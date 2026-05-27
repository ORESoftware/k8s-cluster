package main

import (
	"testing"
	"time"

	"github.com/prometheus/client_golang/prometheus"
	dto "github.com/prometheus/client_model/go"
	appsv1 "k8s.io/api/apps/v1"
	corev1 "k8s.io/api/core/v1"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
)

func dep(name, threadID string, replicas, ready int32, extra map[string]string) appsv1.Deployment {
	labels := map[string]string{
		"app.kubernetes.io/component": "thread-pod",
		"dd/threadId":                 threadID,
		"dd/userId":                   "user-x",
	}
	for k, v := range extra {
		labels[k] = v
	}
	return appsv1.Deployment{
		ObjectMeta: metav1.ObjectMeta{Name: name, Namespace: "dd-dev", Labels: labels},
		Spec:       appsv1.DeploymentSpec{Replicas: &replicas},
		Status:     appsv1.DeploymentStatus{ReadyReplicas: ready},
	}
}

func pod(name, threadID, phase string, ready bool, restarts int32, waiting string) corev1.Pod {
	conds := []corev1.PodCondition{}
	if ready {
		conds = append(conds, corev1.PodCondition{Type: corev1.PodReady, Status: corev1.ConditionTrue})
	}
	cs := corev1.ContainerStatus{Name: "dev-server", RestartCount: restarts}
	if waiting != "" {
		cs.State.Waiting = &corev1.ContainerStateWaiting{Reason: waiting}
	}
	return corev1.Pod{
		ObjectMeta: metav1.ObjectMeta{
			Name:              name,
			Namespace:         "dd-dev",
			Labels:            map[string]string{"dd/threadId": threadID},
			CreationTimestamp: metav1.NewTime(time.Now()),
		},
		Status: corev1.PodStatus{
			Phase:             corev1.PodPhase(phase),
			Conditions:        conds,
			ContainerStatuses: []corev1.ContainerStatus{cs},
		},
	}
}

func gaugeValue(t *testing.T, g prometheus.Gauge) float64 {
	t.Helper()
	var m dto.Metric
	if err := g.Write(&m); err != nil {
		t.Fatalf("write gauge: %v", err)
	}
	return m.Gauge.GetValue()
}

func gaugeVecValue(t *testing.T, gv *prometheus.GaugeVec, lvs ...string) float64 {
	t.Helper()
	g, err := gv.GetMetricWithLabelValues(lvs...)
	if err != nil {
		t.Fatalf("GetMetricWithLabelValues(%v): %v", lvs, err)
	}
	return gaugeValue(t, g)
}

func TestDerivePhase(t *testing.T) {
	cases := []struct {
		name     string
		replicas int32
		pod      corev1.Pod
		want     string
	}{
		{"sleeping by replicas=0", 0, corev1.Pod{}, "sleeping"},
		{"dead missing pod", 1, corev1.Pod{}, "dead"},
		{"failed CrashLoopBackOff", 1, pod("p", "t", "Pending", false, 0, "CrashLoopBackOff"), "failed"},
		{"failed too many restarts", 1, pod("p", "t", "Running", true, 7, ""), "failed"},
		{"starting not ready yet", 1, pod("p", "t", "Pending", false, 0, ""), "starting"},
		{"active running and ready", 1, pod("p", "t", "Running", true, 0, ""), "active"},
	}
	for _, c := range cases {
		t.Run(c.name, func(t *testing.T) {
			d := appsv1.Deployment{Spec: appsv1.DeploymentSpec{Replicas: &c.replicas}}
			got := derivePhase(d, c.pod)
			if got != c.want {
				t.Errorf("derivePhase = %q, want %q", got, c.want)
			}
		})
	}
}

func TestUpdateMetricsAggregates(t *testing.T) {
	reg := prometheus.NewRegistry()
	m := newMetrics(reg)

	deps := []appsv1.Deployment{
		dep("dd-thread-001", "t1", 1, 1, map[string]string{"dd.dev/managed-by": "dd-thread-operator"}),
		dep("dd-thread-002", "t2", 0, 0, nil), // sleeping (template)
		dep("dd-thread-003", "t3", 1, 0, nil), // starting
	}
	pods := []corev1.Pod{
		pod("dd-thread-001-abc", "t1", "Running", true, 0, ""),
		pod("dd-thread-003-xyz", "t3", "Pending", false, 0, ""),
	}
	pvcs := []corev1.PersistentVolumeClaim{
		{ObjectMeta: metav1.ObjectMeta{Name: "dd-thread-001"}, Status: corev1.PersistentVolumeClaimStatus{Phase: corev1.ClaimBound}},
		{ObjectMeta: metav1.ObjectMeta{Name: "dd-thread-002"}, Status: corev1.PersistentVolumeClaimStatus{Phase: corev1.ClaimBound}},
		{ObjectMeta: metav1.ObjectMeta{Name: "dd-thread-003"}, Status: corev1.PersistentVolumeClaimStatus{Phase: corev1.ClaimPending}},
	}

	updateMetrics(m, deps, pods, pvcs)

	if got := gaugeVecValue(t, m.threadFleetTotal, "active"); got != 1 {
		t.Errorf("active = %v, want 1", got)
	}
	if got := gaugeVecValue(t, m.threadFleetTotal, "sleeping"); got != 1 {
		t.Errorf("sleeping = %v, want 1", got)
	}
	if got := gaugeVecValue(t, m.threadFleetTotal, "starting"); got != 1 {
		t.Errorf("starting = %v, want 1", got)
	}
	if got := gaugeVecValue(t, m.threadFleetTotal, "failed"); got != 0 {
		t.Errorf("failed = %v, want 0", got)
	}
	if got := gaugeValue(t, m.replicasDesired); got != 2 {
		t.Errorf("replicasDesired = %v, want 2", got)
	}
	if got := gaugeValue(t, m.replicasReady); got != 1 {
		t.Errorf("replicasReady = %v, want 1", got)
	}
	if got := gaugeVecValue(t, m.pvcStates, "bound"); got != 2 {
		t.Errorf("pvc bound = %v, want 2", got)
	}
	if got := gaugeVecValue(t, m.pvcStates, "pending"); got != 1 {
		t.Errorf("pvc pending = %v, want 1", got)
	}
}
