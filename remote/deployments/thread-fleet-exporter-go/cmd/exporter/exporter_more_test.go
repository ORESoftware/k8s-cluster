package main

// Additional unit tests for the thread-fleet Prometheus exporter.
//
// These complement main_test.go (which covers derivePhase basics and a
// single updateMetrics aggregation case). Here we pin the deterministic,
// cluster-free logic:
//
//   - env/flag config parsing and precedence
//   - the exported metric-family name+type contract (matches the taxonomy
//     that Grafana/alerting hang off of, guarded at source level by
//     remote/tests/general/thread-fleet-exporter-go-config.test.ts)
//   - dd_thread_fleet_threads label construction + the invariant that ONLY
//     the four whitelisted labels are ever exported (no arbitrary/secret
//     Deployment labels leak into /metrics)
//   - threadInfo reset-between-scrapes (deleted threads drop out)
//   - "newest pod wins" pod-dedup logic
//   - the /metrics HTTP handler exposition (httptest, no live cluster)
//   - buildRESTConfig file/in-cluster branches
//
// Everything runs offline and adds NO new module dependencies (scrapeOnce is
// thin glue over updateMetrics + three read-only List calls; unit-testing it
// would require the k8s fake clientset, which would expand this deliberately
// tiny module's dependency graph. Its read-only guarantee is already enforced
// by the RBAC manifest + remote/tests/general/thread-fleet-exporter-go-config.test.ts).

import (
	"errors"
	"flag"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/prometheus/client_golang/prometheus"
	"github.com/prometheus/client_golang/prometheus/promhttp"
	dto "github.com/prometheus/client_model/go"
	appsv1 "k8s.io/api/apps/v1"
	corev1 "k8s.io/api/core/v1"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"k8s.io/client-go/rest"
)

// ---- small local helpers (main_test.go already provides dep/pod/gaugeValue/gaugeVecValue) ----

func ptr32(v int32) *int32 { return &v }

// podAt is pod() with an explicit CreationTimestamp so "newest wins" is deterministic.
func podAt(name, threadID, phase string, ready bool, restarts int32, waiting string, created time.Time) corev1.Pod {
	p := pod(name, threadID, phase, ready, restarts, waiting)
	p.CreationTimestamp = metav1.NewTime(created)
	return p
}

func gatherFamilies(t *testing.T, reg *prometheus.Registry) []*dto.MetricFamily {
	t.Helper()
	mfs, err := reg.Gather()
	if err != nil {
		t.Fatalf("gather: %v", err)
	}
	return mfs
}

// scrapeMetricsHTTP hits the exact handler main() wires up and returns the response.
func scrapeMetricsHTTP(t *testing.T, reg *prometheus.Registry) (status int, contentType, body string) {
	t.Helper()
	h := promhttp.HandlerFor(reg, promhttp.HandlerOpts{Registry: reg})
	rr := httptest.NewRecorder()
	h.ServeHTTP(rr, httptest.NewRequest(http.MethodGet, "/metrics", nil))
	return rr.Code, rr.Header().Get("Content-Type"), rr.Body.String()
}

// threadInfoLabelSets returns the label maps for every dd_thread_fleet_threads series.
func threadInfoLabelSets(t *testing.T, reg *prometheus.Registry) []map[string]string {
	t.Helper()
	var out []map[string]string
	for _, mf := range gatherFamilies(t, reg) {
		if mf.GetName() != "dd_thread_fleet_threads" {
			continue
		}
		for _, m := range mf.Metric {
			ls := map[string]string{}
			for _, lp := range m.Label {
				ls[lp.GetName()] = lp.GetValue()
			}
			out = append(out, ls)
		}
	}
	return out
}

// ---- config: envOr ----

func TestEnvOr(t *testing.T) {
	const key = "THREAD_FLEET_TEST_ENVOR_KEY" // unlikely to be set in any environment
	cases := []struct {
		name     string
		set      bool
		value    string
		fallback string
		want     string
	}{
		{"unset returns fallback", false, "", "fb", "fb"},
		{"empty value returns fallback", true, "", "fb", "fb"},
		{"non-empty value wins", true, "chosen", "fb", "chosen"},
		{"whitespace value is non-empty and wins", true, " ", "fb", " "},
	}
	for _, c := range cases {
		t.Run(c.name, func(t *testing.T) {
			os.Unsetenv(key)
			if c.set {
				t.Setenv(key, c.value)
			}
			if got := envOr(key, c.fallback); got != c.want {
				t.Errorf("envOr(%q,%q) = %q, want %q", key, c.fallback, got, c.want)
			}
		})
	}
}

// ---- config: parseFlags precedence (default -> env -> CLI flag) ----

// withCleanFlags isolates the global flag/args state so parseFlags can be
// exercised without leaking into other tests or picking up `go test` flags.
func withCleanFlags(t *testing.T, args ...string) {
	t.Helper()
	oldArgs := os.Args
	oldCL := flag.CommandLine
	t.Cleanup(func() {
		os.Args = oldArgs
		flag.CommandLine = oldCL
	})
	flag.CommandLine = flag.NewFlagSet("exporter", flag.ContinueOnError)
	os.Args = append([]string{"exporter"}, args...)
}

func TestParseFlagsDefaults(t *testing.T) {
	// Guarantee the env-backed defaults are actually the compiled-in defaults.
	for _, k := range []string{"THREAD_FLEET_NAMESPACE", "THREAD_FLEET_LABEL_SELECTOR", "KUBECONFIG"} {
		orig, had := os.LookupEnv(k)
		os.Unsetenv(k)
		if had {
			t.Cleanup(func() { os.Setenv(k, orig) })
		}
	}
	withCleanFlags(t)

	cfg := parseFlags()
	if cfg.listenAddr != ":9103" {
		t.Errorf("listenAddr = %q, want :9103", cfg.listenAddr)
	}
	if cfg.namespace != defaultNamespace {
		t.Errorf("namespace = %q, want default %q", cfg.namespace, defaultNamespace)
	}
	if cfg.labelSelector != defaultLabelSelector {
		t.Errorf("labelSelector = %q, want default %q", cfg.labelSelector, defaultLabelSelector)
	}
	if cfg.scrapePeriod != 15*time.Second {
		t.Errorf("scrapePeriod = %v, want 15s", cfg.scrapePeriod)
	}
	if cfg.kubeconfig != "" {
		t.Errorf("kubeconfig = %q, want empty", cfg.kubeconfig)
	}
}

func TestParseFlagsEnvProvidesDefault(t *testing.T) {
	t.Setenv("THREAD_FLEET_NAMESPACE", "env-ns")
	t.Setenv("THREAD_FLEET_LABEL_SELECTOR", "team=env")
	withCleanFlags(t) // no CLI overrides -> env-backed defaults win

	cfg := parseFlags()
	if cfg.namespace != "env-ns" {
		t.Errorf("namespace = %q, want env-ns (from env)", cfg.namespace)
	}
	if cfg.labelSelector != "team=env" {
		t.Errorf("labelSelector = %q, want team=env (from env)", cfg.labelSelector)
	}
}

func TestParseFlagsCLIOverridesEnv(t *testing.T) {
	t.Setenv("THREAD_FLEET_NAMESPACE", "env-ns")
	t.Setenv("THREAD_FLEET_LABEL_SELECTOR", "team=env")
	withCleanFlags(t,
		"--namespace=cli-ns",
		"--listen-addr=:9999",
		"--scrape-period=30s",
		"--kubeconfig=/tmp/kc",
	)

	cfg := parseFlags()
	if cfg.namespace != "cli-ns" {
		t.Errorf("namespace = %q, want cli-ns (CLI beats env)", cfg.namespace)
	}
	if cfg.listenAddr != ":9999" {
		t.Errorf("listenAddr = %q, want :9999", cfg.listenAddr)
	}
	if cfg.scrapePeriod != 30*time.Second {
		t.Errorf("scrapePeriod = %v, want 30s", cfg.scrapePeriod)
	}
	if cfg.kubeconfig != "/tmp/kc" {
		t.Errorf("kubeconfig = %q, want /tmp/kc", cfg.kubeconfig)
	}
	// label-selector had no CLI flag, so it stays at the env-provided default.
	if cfg.labelSelector != "team=env" {
		t.Errorf("labelSelector = %q, want team=env (unoverridden env default)", cfg.labelSelector)
	}
}

// ---- newMetrics: taxonomies pre-zeroed, and the exact family name+type contract ----

func TestNewMetricsZeroesTaxonomies(t *testing.T) {
	reg := prometheus.NewRegistry()
	m := newMetrics(reg)

	// Every documented phase pre-initialized to 0 so a series exists before
	// the first scrape (rate()/alerting doesn't see gaps on cold start).
	for _, phase := range []string{"active", "starting", "sleeping", "failed", "dead"} {
		if got := gaugeVecValue(t, m.threadFleetTotal, phase); got != 0 {
			t.Errorf("threadFleetTotal[%s] = %v, want 0", phase, got)
		}
	}
	for _, state := range []string{"bound", "pending", "lost", "unknown"} {
		if got := gaugeVecValue(t, m.pvcStates, state); got != 0 {
			t.Errorf("pvcStates[%s] = %v, want 0", state, got)
		}
	}
}

// TestExportedMetricFamilyContract pins the metric names AND their Prometheus
// types. This is the runtime counterpart to the source-level regex assertions
// in the TS contract test. NOTE: the four gauges carrying a `_total` suffix
// (dd_thread_fleet_total, _replicas_desired_total, _replicas_ready_total,
// _pvcs_total) deviate from Prometheus naming convention (_total is reserved
// for counters); this test documents that as the current, intentional contract
// rather than asserting the convention.
func TestExportedMetricFamilyContract(t *testing.T) {
	reg := prometheus.NewRegistry()
	m := newMetrics(reg)
	// Materialize a threadInfo series so its family shows up in Gather().
	updateMetrics(m,
		[]appsv1.Deployment{dep("dd-thread-001", "t1", 1, 1, nil)},
		[]corev1.Pod{pod("dd-thread-001-a", "t1", "Running", true, 0, "")},
		nil,
	)

	want := map[string]string{
		"dd_thread_fleet_total":                  "GAUGE",
		"dd_thread_fleet_replicas_desired_total": "GAUGE",
		"dd_thread_fleet_replicas_ready_total":   "GAUGE",
		"dd_thread_fleet_pvcs_total":             "GAUGE",
		"dd_thread_fleet_threads":                "GAUGE",
		"dd_thread_fleet_scrape_seconds":         "HISTOGRAM",
		"dd_thread_fleet_scrape_errors_total":    "COUNTER",
	}
	got := map[string]string{}
	for _, mf := range gatherFamilies(t, reg) {
		got[mf.GetName()] = mf.GetType().String()
	}
	if len(got) != len(want) {
		t.Errorf("family count = %d, want %d; got=%v", len(got), len(want), got)
	}
	for name, typ := range want {
		if got[name] != typ {
			t.Errorf("family %q type = %q, want %q", name, got[name], typ)
		}
	}
	for name := range got {
		if _, ok := want[name]; !ok {
			t.Errorf("unexpected exported family %q", name)
		}
	}
}

// ---- derivePhase: edge cases beyond main_test.go's basic set ----

func TestDerivePhaseEdgeCases(t *testing.T) {
	activePod := pod("p", "t", "Running", true, 0, "")

	multiContainerOverLimit := corev1.Pod{
		ObjectMeta: metav1.ObjectMeta{
			Name:              "p-multi",
			Namespace:         "dd-dev",
			Labels:            map[string]string{"dd/threadId": "t"},
			CreationTimestamp: metav1.NewTime(time.Now()),
		},
		Status: corev1.PodStatus{
			Phase:      corev1.PodRunning,
			Conditions: []corev1.PodCondition{{Type: corev1.PodReady, Status: corev1.ConditionTrue}},
			// Restart counts are SUMMED across containers: 3+3 = 6 > 5 -> failed.
			ContainerStatuses: []corev1.ContainerStatus{
				{Name: "a", RestartCount: 3},
				{Name: "b", RestartCount: 3},
			},
		},
	}

	cases := []struct {
		name     string
		replicas *int32
		pod      corev1.Pod
		want     string
	}{
		{"nil replicas -> sleeping (even with a healthy pod)", nil, activePod, "sleeping"},
		{"replicas 0 -> sleeping", ptr32(0), corev1.Pod{}, "sleeping"},
		{"ImagePullBackOff -> failed", ptr32(1), pod("p", "t", "Pending", false, 0, "ImagePullBackOff"), "failed"},
		{"ErrImagePull -> failed", ptr32(1), pod("p", "t", "Pending", false, 0, "ErrImagePull"), "failed"},
		{"restarts exactly 5 is not failed (boundary is >5)", ptr32(1), pod("p", "t", "Running", true, 5, ""), "active"},
		{"summed restarts across containers >5 -> failed", ptr32(1), multiContainerOverLimit, "failed"},
		{"running but not ready -> starting", ptr32(1), pod("p", "t", "Running", false, 0, ""), "starting"},
		{"benign waiting reason -> starting", ptr32(1), pod("p", "t", "Pending", false, 0, "ContainerCreating"), "starting"},
	}
	for _, c := range cases {
		t.Run(c.name, func(t *testing.T) {
			d := appsv1.Deployment{Spec: appsv1.DeploymentSpec{Replicas: c.replicas}}
			if got := derivePhase(d, c.pod); got != c.want {
				t.Errorf("derivePhase = %q, want %q", got, c.want)
			}
		})
	}
}

// TestDerivePhaseTaxonomyClosed: derivePhase must never return a phase outside
// the documented set, regardless of input. Grafana/alert rules assume this.
func TestDerivePhaseTaxonomyClosed(t *testing.T) {
	allowed := map[string]bool{"active": true, "starting": true, "sleeping": true, "failed": true, "dead": true}
	pods := []corev1.Pod{
		{},
		pod("p", "t", "Running", true, 0, ""),
		pod("p", "t", "Running", false, 99, "CrashLoopBackOff"),
		pod("p", "t", "Pending", false, 0, "ContainerCreating"),
		pod("p", "t", "Failed", false, 0, ""),
		pod("p", "t", "Unknown", false, 0, "Weird"),
	}
	for _, replicas := range []*int32{nil, ptr32(0), ptr32(1), ptr32(3)} {
		for _, p := range pods {
			d := appsv1.Deployment{Spec: appsv1.DeploymentSpec{Replicas: replicas}}
			got := derivePhase(d, p)
			if !allowed[got] {
				t.Errorf("derivePhase returned out-of-taxonomy phase %q (replicas=%v)", got, replicas)
			}
		}
	}
}

// ---- updateMetrics: label construction + reset + newest-pod-wins ----

func TestUpdateMetricsThreadInfoLabels(t *testing.T) {
	reg := prometheus.NewRegistry()
	m := newMetrics(reg)

	deps := []appsv1.Deployment{
		// operator-managed: managed_by label carried through verbatim.
		dep("dd-thread-abc", "tid-abc", 1, 1, map[string]string{"dd.dev/managed-by": "dd-thread-operator"}),
		// template path: no managed-by label -> defaults to "template".
		dep("dd-thread-def", "tid-def", 1, 1, nil),
		// name without the dd-thread- prefix -> short == full name.
		dep("legacy-name", "tid-legacy", 1, 1, nil),
	}
	pods := []corev1.Pod{
		pod("dd-thread-abc-1", "tid-abc", "Running", true, 0, ""),
		pod("dd-thread-def-1", "tid-def", "Running", true, 0, ""),
		pod("legacy-name-1", "tid-legacy", "Running", true, 0, ""),
	}
	updateMetrics(m, deps, pods, nil)

	// short = strings.TrimPrefix(Deployment.Name, "dd-thread-"); userId is "user-x" (set by dep()).
	if v := gaugeVecValue(t, m.threadInfo, "abc", "tid-abc", "user-x", "dd-thread-operator"); v != 1 {
		t.Errorf("operator threadInfo series = %v, want 1", v)
	}
	if v := gaugeVecValue(t, m.threadInfo, "def", "tid-def", "user-x", "template"); v != 1 {
		t.Errorf("template threadInfo series = %v, want 1 (managed_by must default to template)", v)
	}
	if v := gaugeVecValue(t, m.threadInfo, "legacy-name", "tid-legacy", "user-x", "template"); v != 1 {
		t.Errorf("legacy-name threadInfo series = %v, want 1 (short == full name when no prefix)", v)
	}
}

// TestThreadInfoNoArbitraryLabelLeak encodes the cardinality/PII contract from
// the readme: dd_thread_fleet_threads exposes ONLY the four whitelisted labels
// (thread_id_short, thread_id, user_id, managed_by). Arbitrary Deployment
// labels -- including anything secret-shaped -- must never be copied into the
// metric. Regression guard: if someone later reflects all labels, this fails.
func TestThreadInfoNoArbitraryLabelLeak(t *testing.T) {
	reg := prometheus.NewRegistry()
	m := newMetrics(reg)

	d := dep("dd-thread-secret", "tid-1", 1, 1, map[string]string{
		"dd/sessionToken":    "SUPER-SECRET-TOKEN-VALUE",
		"internal/pii-email": "person@example.com",
		"dd.dev/managed-by":  "dd-thread-operator",
	})
	updateMetrics(m,
		[]appsv1.Deployment{d},
		[]corev1.Pod{pod("dd-thread-secret-1", "tid-1", "Running", true, 0, "")},
		nil,
	)

	// Exactly one series, and its label keys are exactly the four whitelisted ones.
	sets := threadInfoLabelSets(t, reg)
	if len(sets) != 1 {
		t.Fatalf("threadInfo series count = %d, want 1", len(sets))
	}
	wantKeys := map[string]bool{"thread_id_short": true, "thread_id": true, "user_id": true, "managed_by": true}
	for k := range sets[0] {
		if !wantKeys[k] {
			t.Errorf("unexpected label %q on dd_thread_fleet_threads (possible label leak)", k)
		}
	}
	for k := range wantKeys {
		if _, ok := sets[0][k]; !ok {
			t.Errorf("missing expected label %q on dd_thread_fleet_threads", k)
		}
	}

	// And the secret/PII values must be nowhere in the exposition output.
	_, _, body := scrapeMetricsHTTP(t, reg)
	for _, needle := range []string{"SUPER-SECRET-TOKEN-VALUE", "person@example.com", "sessionToken", "pii-email"} {
		if strings.Contains(body, needle) {
			t.Errorf("/metrics output leaked %q", needle)
		}
	}
}

// TestUpdateMetricsResetsThreadInfoBetweenScrapes: the readme promises deleted
// threads drop out of dd_thread_fleet_threads because it's Reset() each scrape.
func TestUpdateMetricsResetsThreadInfoBetweenScrapes(t *testing.T) {
	reg := prometheus.NewRegistry()
	m := newMetrics(reg)

	updateMetrics(m, []appsv1.Deployment{
		dep("dd-thread-001", "t1", 1, 1, nil),
		dep("dd-thread-002", "t2", 1, 1, nil),
	}, nil, nil)
	if got := len(threadInfoLabelSets(t, reg)); got != 2 {
		t.Fatalf("after scrape 1: threadInfo series = %d, want 2", got)
	}

	// Second scrape sees only t1 (t2 "deleted"): t2's series must vanish.
	updateMetrics(m, []appsv1.Deployment{
		dep("dd-thread-001", "t1", 1, 1, nil),
	}, nil, nil)
	sets := threadInfoLabelSets(t, reg)
	if len(sets) != 1 {
		t.Fatalf("after scrape 2: threadInfo series = %d, want 1 (deleted thread must drop out)", len(sets))
	}
	if sets[0]["thread_id"] != "t1" {
		t.Errorf("surviving series thread_id = %q, want t1", sets[0]["thread_id"])
	}
}

// TestUpdateMetricsNewestPodWins: for a given threadId the newest Pod (by
// CreationTimestamp) decides the phase, regardless of slice order.
func TestUpdateMetricsNewestPodWins(t *testing.T) {
	base := time.Date(2026, 1, 1, 0, 0, 0, 0, time.UTC)
	older := podAt("dd-thread-1-old", "t1", "Running", true, 0, "", base)                 // would be "active"
	newer := podAt("dd-thread-1-new", "t1", "Pending", false, 0, "", base.Add(time.Hour)) // would be "starting"

	for _, order := range []struct {
		name string
		pods []corev1.Pod
	}{
		{"older first", []corev1.Pod{older, newer}},
		{"newer first", []corev1.Pod{newer, older}},
	} {
		t.Run(order.name, func(t *testing.T) {
			reg := prometheus.NewRegistry()
			m := newMetrics(reg)
			updateMetrics(m, []appsv1.Deployment{dep("dd-thread-1", "t1", 1, 0, nil)}, order.pods, nil)
			// Newer pod is Pending/not-ready -> "starting"; the older active pod must lose.
			if got := gaugeVecValue(t, m.threadFleetTotal, "starting"); got != 1 {
				t.Errorf("starting = %v, want 1 (newest pod should win)", got)
			}
			if got := gaugeVecValue(t, m.threadFleetTotal, "active"); got != 0 {
				t.Errorf("active = %v, want 0 (older pod must not win)", got)
			}
		})
	}
}

// TestUpdateMetricsPodWithoutThreadIdSkipped: a Pod lacking the dd/threadId
// label is ignored, so a Deployment expecting it is counted "dead".
func TestUpdateMetricsPodWithoutThreadIdSkipped(t *testing.T) {
	reg := prometheus.NewRegistry()
	m := newMetrics(reg)

	unlabeled := pod("dd-thread-1-x", "", "Running", true, 0, "") // pod() with "" clears dd/threadId
	unlabeled.Labels = map[string]string{}                        // ensure no threadId label at all

	updateMetrics(m, []appsv1.Deployment{dep("dd-thread-1", "t1", 1, 0, nil)}, []corev1.Pod{unlabeled}, nil)

	if got := gaugeVecValue(t, m.threadFleetTotal, "dead"); got != 1 {
		t.Errorf("dead = %v, want 1 (unlabeled pod must not satisfy the deployment)", got)
	}
	if got := gaugeVecValue(t, m.threadFleetTotal, "active"); got != 0 {
		t.Errorf("active = %v, want 0", got)
	}
}

// TestUpdateMetricsPVCStates: all four buckets, including the default->unknown fallthrough.
func TestUpdateMetricsPVCStates(t *testing.T) {
	reg := prometheus.NewRegistry()
	m := newMetrics(reg)

	pvc := func(name string, phase corev1.PersistentVolumeClaimPhase) corev1.PersistentVolumeClaim {
		return corev1.PersistentVolumeClaim{
			ObjectMeta: metav1.ObjectMeta{Name: name, Namespace: "dd-dev"},
			Status:     corev1.PersistentVolumeClaimStatus{Phase: phase},
		}
	}
	pvcs := []corev1.PersistentVolumeClaim{
		pvc("a", corev1.ClaimBound),
		pvc("b", corev1.ClaimBound),
		pvc("c", corev1.ClaimPending),
		pvc("d", corev1.ClaimLost),
		pvc("e", corev1.PersistentVolumeClaimPhase("SomethingWeird")), // default -> unknown
		pvc("f", corev1.PersistentVolumeClaimPhase("")),               // empty phase -> unknown
	}
	updateMetrics(m, nil, nil, pvcs)

	for state, want := range map[string]float64{"bound": 2, "pending": 1, "lost": 1, "unknown": 2} {
		if got := gaugeVecValue(t, m.pvcStates, state); got != want {
			t.Errorf("pvcStates[%s] = %v, want %v", state, got, want)
		}
	}
}

// ---- /metrics HTTP handler exposition ----

func TestMetricsHTTPHandlerExposition(t *testing.T) {
	reg := prometheus.NewRegistry()
	m := newMetrics(reg)
	updateMetrics(m,
		[]appsv1.Deployment{dep("dd-thread-001", "t1", 1, 1, nil)},
		[]corev1.Pod{pod("dd-thread-001-a", "t1", "Running", true, 0, "")},
		[]corev1.PersistentVolumeClaim{{
			ObjectMeta: metav1.ObjectMeta{Name: "dd-thread-001"},
			Status:     corev1.PersistentVolumeClaimStatus{Phase: corev1.ClaimBound},
		}},
	)

	status, ct, body := scrapeMetricsHTTP(t, reg)
	if status != http.StatusOK {
		t.Fatalf("status = %d, want 200", status)
	}
	if !strings.HasPrefix(ct, "text/plain") {
		t.Errorf("Content-Type = %q, want text/plain exposition format", ct)
	}

	// Prometheus text format sorts label names alphabetically:
	// managed_by < thread_id < thread_id_short < user_id.
	wantSubstrings := []string{
		"# TYPE dd_thread_fleet_total gauge",
		`dd_thread_fleet_total{phase="active"} 1`,
		"# TYPE dd_thread_fleet_threads gauge",
		`dd_thread_fleet_threads{managed_by="template",thread_id="t1",thread_id_short="001",user_id="user-x"} 1`,
		`dd_thread_fleet_pvcs_total{state="bound"} 1`,
		"# TYPE dd_thread_fleet_scrape_seconds histogram",
		"dd_thread_fleet_scrape_errors_total 0",
		"dd_thread_fleet_replicas_desired_total 1",
		"dd_thread_fleet_replicas_ready_total 1",
	}
	for _, s := range wantSubstrings {
		if !strings.Contains(body, s) {
			t.Errorf("/metrics body missing %q\n---body---\n%s", s, body)
		}
	}
}

// ---- buildRESTConfig ----

func TestBuildRESTConfigFromKubeconfigFile(t *testing.T) {
	const kubeconfig = `apiVersion: v1
kind: Config
clusters:
- name: test-cluster
  cluster:
    server: https://example.test:6443
    insecure-skip-tls-verify: true
contexts:
- name: test-context
  context:
    cluster: test-cluster
    user: test-user
current-context: test-context
users:
- name: test-user
  user:
    token: test-token
`
	path := filepath.Join(t.TempDir(), "kubeconfig")
	if err := os.WriteFile(path, []byte(kubeconfig), 0o600); err != nil {
		t.Fatalf("write kubeconfig: %v", err)
	}
	cfg, err := buildRESTConfig(path)
	if err != nil {
		t.Fatalf("buildRESTConfig(path): %v", err)
	}
	if cfg.Host != "https://example.test:6443" {
		t.Errorf("Host = %q, want https://example.test:6443", cfg.Host)
	}
}

func TestBuildRESTConfigMissingFile(t *testing.T) {
	if _, err := buildRESTConfig(filepath.Join(t.TempDir(), "does-not-exist")); err == nil {
		t.Error("buildRESTConfig(missing) returned nil error, want failure")
	}
}

func TestBuildRESTConfigEmptyIsInCluster(t *testing.T) {
	if os.Getenv("KUBERNETES_SERVICE_HOST") != "" {
		t.Skip("running inside a cluster; in-cluster config would succeed")
	}
	_, err := buildRESTConfig("")
	if !errors.Is(err, rest.ErrNotInCluster) {
		t.Errorf("buildRESTConfig(\"\") err = %v, want rest.ErrNotInCluster", err)
	}
}
