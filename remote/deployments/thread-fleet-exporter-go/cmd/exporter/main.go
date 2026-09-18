// Command exporter is a small Prometheus exporter for the dd-dev
// thread fleet.
//
// What it exports:
//
//   - dd_thread_fleet_total{phase="active|starting|sleeping|failed|dead"}
//     Gauge with the same lifecycle taxonomy used by /u/admin/k8s.
//   - dd_thread_fleet_replicas_desired_total
//   - dd_thread_fleet_replicas_ready_total
//   - dd_thread_fleet_pvcs_total{state="bound|pending|lost"}
//   - dd_thread_fleet_threads{thread_id_short="...",thread_id="...",user_id="...",managed_by="..."}
//     1 per known thread, labels carry identifying info. Cardinality
//     is bounded by the number of live thread Deployments (low-tens
//     today, hundreds in steady-state).
//   - dd_thread_fleet_scrape_seconds — exporter self-timing histogram.
//   - dd_thread_fleet_scrape_errors_total — failed-scrape counter.
//
// Scope: namespace = dd-dev, label selector =
// app.kubernetes.io/component=thread-pod (matches both the existing
// template path and the operator path).
//
// Read-only. Does not call any write API.
package main

import (
	"context"
	"flag"
	"fmt"
	"net/http"
	"os"
	"os/signal"
	"strings"
	"syscall"
	"time"

	"github.com/prometheus/client_golang/prometheus"
	"github.com/prometheus/client_golang/prometheus/promhttp"
	appsv1 "k8s.io/api/apps/v1"
	corev1 "k8s.io/api/core/v1"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"k8s.io/client-go/kubernetes"
	"k8s.io/client-go/rest"
	"k8s.io/client-go/tools/clientcmd"
)

const (
	defaultNamespace     = "dd-dev"
	defaultLabelSelector = "app.kubernetes.io/component=thread-pod"
)

type config struct {
	listenAddr    string
	namespace     string
	labelSelector string
	scrapePeriod  time.Duration
	kubeconfig    string
}

func parseFlags() config {
	var c config
	flag.StringVar(&c.listenAddr, "listen-addr", ":9103", "address to bind /metrics on")
	flag.StringVar(&c.namespace, "namespace", envOr("THREAD_FLEET_NAMESPACE", defaultNamespace), "namespace to watch")
	flag.StringVar(&c.labelSelector, "label-selector", envOr("THREAD_FLEET_LABEL_SELECTOR", defaultLabelSelector), "label selector for thread Deployments and Pods")
	flag.DurationVar(&c.scrapePeriod, "scrape-period", 15*time.Second, "how often to refresh internal counters")
	flag.StringVar(&c.kubeconfig, "kubeconfig", os.Getenv("KUBECONFIG"), "path to kubeconfig for out-of-cluster runs (empty -> in-cluster)")
	flag.Parse()
	return c
}

func envOr(key, fallback string) string {
	if v := os.Getenv(key); v != "" {
		return v
	}
	return fallback
}

type metrics struct {
	threadFleetTotal     *prometheus.GaugeVec
	replicasDesired      prometheus.Gauge
	replicasReady        prometheus.Gauge
	pvcStates            *prometheus.GaugeVec
	threadInfo           *prometheus.GaugeVec
	scrapeSeconds        prometheus.Histogram
	scrapeErrors         prometheus.Counter
}

func newMetrics(reg prometheus.Registerer) *metrics {
	m := &metrics{
		threadFleetTotal: prometheus.NewGaugeVec(prometheus.GaugeOpts{
			Name: "dd_thread_fleet_total",
			Help: "Number of dd-dev thread Deployments by derived lifecycle phase.",
		}, []string{"phase"}),
		replicasDesired: prometheus.NewGauge(prometheus.GaugeOpts{
			Name: "dd_thread_fleet_replicas_desired_total",
			Help: "Sum of spec.replicas across all thread Deployments.",
		}),
		replicasReady: prometheus.NewGauge(prometheus.GaugeOpts{
			Name: "dd_thread_fleet_replicas_ready_total",
			Help: "Sum of status.readyReplicas across all thread Deployments.",
		}),
		pvcStates: prometheus.NewGaugeVec(prometheus.GaugeOpts{
			Name: "dd_thread_fleet_pvcs_total",
			Help: "Number of dd-dev thread PVCs by phase.",
		}, []string{"state"}),
		threadInfo: prometheus.NewGaugeVec(prometheus.GaugeOpts{
			Name: "dd_thread_fleet_threads",
			Help: "1 per dd-dev thread Deployment, labels carry identifying info.",
		}, []string{"thread_id_short", "thread_id", "user_id", "managed_by"}),
		scrapeSeconds: prometheus.NewHistogram(prometheus.HistogramOpts{
			Name:    "dd_thread_fleet_scrape_seconds",
			Help:    "Wall-clock seconds spent on each fleet refresh.",
			Buckets: prometheus.ExponentialBuckets(0.05, 2, 8),
		}),
		scrapeErrors: prometheus.NewCounter(prometheus.CounterOpts{
			Name: "dd_thread_fleet_scrape_errors_total",
			Help: "Number of fleet refreshes that failed.",
		}),
	}
	reg.MustRegister(
		m.threadFleetTotal,
		m.replicasDesired,
		m.replicasReady,
		m.pvcStates,
		m.threadInfo,
		m.scrapeSeconds,
		m.scrapeErrors,
	)
	for _, phase := range []string{"active", "starting", "sleeping", "failed", "dead"} {
		m.threadFleetTotal.WithLabelValues(phase).Set(0)
	}
	for _, state := range []string{"bound", "pending", "lost", "unknown"} {
		m.pvcStates.WithLabelValues(state).Set(0)
	}
	return m
}

func main() {
	cfg := parseFlags()

	restCfg, err := buildRESTConfig(cfg.kubeconfig)
	if err != nil {
		fmt.Fprintf(os.Stderr, "kube config: %v\n", err)
		os.Exit(1)
	}
	cs, err := kubernetes.NewForConfig(restCfg)
	if err != nil {
		fmt.Fprintf(os.Stderr, "kube client: %v\n", err)
		os.Exit(1)
	}

	reg := prometheus.NewRegistry()
	reg.MustRegister(prometheus.NewGoCollector())
	reg.MustRegister(prometheus.NewProcessCollector(prometheus.ProcessCollectorOpts{}))
	m := newMetrics(reg)

	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()

	// Initial scrape so /metrics has data before the first tick.
	if err := scrapeOnce(ctx, cs, cfg, m); err != nil {
		fmt.Fprintf(os.Stderr, "initial scrape: %v\n", err)
	}

	go runScrapeLoop(ctx, cs, cfg, m)

	mux := http.NewServeMux()
	mux.Handle("/metrics", promhttp.HandlerFor(reg, promhttp.HandlerOpts{Registry: reg}))
	mux.HandleFunc("/healthz", func(w http.ResponseWriter, _ *http.Request) {
		w.WriteHeader(http.StatusOK)
		_, _ = w.Write([]byte("ok"))
	})

	srv := &http.Server{Addr: cfg.listenAddr, Handler: mux, ReadHeaderTimeout: 5 * time.Second}
	go func() {
		fmt.Fprintf(os.Stderr, "listening on %s\n", cfg.listenAddr)
		if err := srv.ListenAndServe(); err != nil && err != http.ErrServerClosed {
			fmt.Fprintf(os.Stderr, "http: %v\n", err)
			os.Exit(1)
		}
	}()

	sig := make(chan os.Signal, 1)
	signal.Notify(sig, syscall.SIGINT, syscall.SIGTERM)
	<-sig
	cancel()
	shutdown, sc := context.WithTimeout(context.Background(), 10*time.Second)
	defer sc()
	_ = srv.Shutdown(shutdown)
}

func buildRESTConfig(kubeconfig string) (*rest.Config, error) {
	if kubeconfig != "" {
		return clientcmd.BuildConfigFromFlags("", kubeconfig)
	}
	return rest.InClusterConfig()
}

func runScrapeLoop(ctx context.Context, cs kubernetes.Interface, cfg config, m *metrics) {
	t := time.NewTicker(cfg.scrapePeriod)
	defer t.Stop()
	for {
		select {
		case <-ctx.Done():
			return
		case <-t.C:
			if err := scrapeOnce(ctx, cs, cfg, m); err != nil {
				m.scrapeErrors.Inc()
				fmt.Fprintf(os.Stderr, "scrape: %v\n", err)
			}
		}
	}
}

func scrapeOnce(ctx context.Context, cs kubernetes.Interface, cfg config, m *metrics) error {
	start := time.Now()
	defer func() { m.scrapeSeconds.Observe(time.Since(start).Seconds()) }()

	listOpts := metav1.ListOptions{LabelSelector: cfg.labelSelector}

	deps, err := cs.AppsV1().Deployments(cfg.namespace).List(ctx, listOpts)
	if err != nil {
		return fmt.Errorf("list deployments: %w", err)
	}
	pods, err := cs.CoreV1().Pods(cfg.namespace).List(ctx, listOpts)
	if err != nil {
		return fmt.Errorf("list pods: %w", err)
	}
	pvcs, err := cs.CoreV1().PersistentVolumeClaims(cfg.namespace).List(ctx, listOpts)
	if err != nil {
		return fmt.Errorf("list pvcs: %w", err)
	}

	updateMetrics(m, deps.Items, pods.Items, pvcs.Items)
	return nil
}

// updateMetrics is the pure function: input k8s objects, output
// gauges. Kept testable.
func updateMetrics(m *metrics, deps []appsv1.Deployment, pods []corev1.Pod, pvcs []corev1.PersistentVolumeClaim) {
	phaseCounts := map[string]int{"active": 0, "starting": 0, "sleeping": 0, "failed": 0, "dead": 0}
	var totalDesired, totalReady int32

	podsByThread := map[string]corev1.Pod{}
	for _, p := range pods {
		tid := p.Labels["dd/threadId"]
		if tid == "" {
			continue
		}
		// Newest pod wins for a given threadId.
		if existing, ok := podsByThread[tid]; ok {
			if p.CreationTimestamp.After(existing.CreationTimestamp.Time) {
				podsByThread[tid] = p
			}
			continue
		}
		podsByThread[tid] = p
	}

	m.threadInfo.Reset()

	for _, d := range deps {
		desired := int32(0)
		if d.Spec.Replicas != nil {
			desired = *d.Spec.Replicas
		}
		ready := d.Status.ReadyReplicas
		totalDesired += desired
		totalReady += ready

		threadID := d.Labels["dd/threadId"]
		userID := d.Labels["dd/userId"]
		short := strings.TrimPrefix(d.Name, "dd-thread-")
		managedBy := d.Labels["dd.dev/managed-by"]
		if managedBy == "" {
			managedBy = "template"
		}

		phase := derivePhase(d, podsByThread[threadID])
		phaseCounts[phase]++
		m.threadInfo.WithLabelValues(short, threadID, userID, managedBy).Set(1)
	}

	for phase, n := range phaseCounts {
		m.threadFleetTotal.WithLabelValues(phase).Set(float64(n))
	}
	m.replicasDesired.Set(float64(totalDesired))
	m.replicasReady.Set(float64(totalReady))

	pvcCounts := map[string]int{"bound": 0, "pending": 0, "lost": 0, "unknown": 0}
	for _, p := range pvcs {
		switch p.Status.Phase {
		case corev1.ClaimBound:
			pvcCounts["bound"]++
		case corev1.ClaimPending:
			pvcCounts["pending"]++
		case corev1.ClaimLost:
			pvcCounts["lost"]++
		default:
			pvcCounts["unknown"]++
		}
	}
	for state, n := range pvcCounts {
		m.pvcStates.WithLabelValues(state).Set(float64(n))
	}
}

// derivePhase mirrors the lifecycle taxonomy used by /u/admin/k8s.
//
//   - sleeping: spec.replicas=0
//   - dead: no Pod found at all
//   - failed: pod restartCount > 5 OR CrashLoopBackOff
//   - starting: pod not yet Ready
//   - active: pod Running + Ready
func derivePhase(dep appsv1.Deployment, pod corev1.Pod) string {
	desired := int32(0)
	if dep.Spec.Replicas != nil {
		desired = *dep.Spec.Replicas
	}
	if desired == 0 {
		return "sleeping"
	}
	if pod.Name == "" {
		return "dead"
	}
	totalRestarts := int32(0)
	for _, cs := range pod.Status.ContainerStatuses {
		totalRestarts += cs.RestartCount
		if waiting := cs.State.Waiting; waiting != nil {
			if waiting.Reason == "CrashLoopBackOff" || waiting.Reason == "ImagePullBackOff" || waiting.Reason == "ErrImagePull" {
				return "failed"
			}
		}
	}
	if totalRestarts > 5 {
		return "failed"
	}
	if pod.Status.Phase == corev1.PodRunning && podReady(pod) {
		return "active"
	}
	return "starting"
}

func podReady(p corev1.Pod) bool {
	for _, c := range p.Status.Conditions {
		if c.Type == corev1.PodReady && c.Status == corev1.ConditionTrue {
			return true
		}
	}
	return false
}
