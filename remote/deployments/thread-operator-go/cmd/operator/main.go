// Command operator runs the dd-thread-operator controller manager.
//
// Configuration is via flags + env. Flags are intentionally minimal —
// the operator is meant to be tuned through Thread spec fields and
// per-cluster RBAC, not a sprawling flag set.
package main

import (
	"context"
	"flag"
	"fmt"
	"os"

	telemetry "github.com/oresoftware/dd/libs/telemetry-go"
	"k8s.io/apimachinery/pkg/runtime"
	utilruntime "k8s.io/apimachinery/pkg/util/runtime"
	clientgoscheme "k8s.io/client-go/kubernetes/scheme"
	ctrl "sigs.k8s.io/controller-runtime"
	"sigs.k8s.io/controller-runtime/pkg/cache"
	"sigs.k8s.io/controller-runtime/pkg/healthz"
	"sigs.k8s.io/controller-runtime/pkg/log/zap"
	metricsserver "sigs.k8s.io/controller-runtime/pkg/metrics/server"

	threadv1 "github.com/ORESoftware/k8s-cluster/remote/deployments/thread-operator-go/api/v1alpha1"
	"github.com/ORESoftware/k8s-cluster/remote/deployments/thread-operator-go/internal/controller"
)

var scheme = runtime.NewScheme()

func init() {
	utilruntime.Must(clientgoscheme.AddToScheme(scheme))
	utilruntime.Must(threadv1.AddToScheme(scheme))
}

func main() {
	var (
		metricsAddr           string
		probeAddr             string
		enableLeaderElection  bool
		watchNamespace        string
	)
	flag.StringVar(&metricsAddr, "metrics-bind-address", ":9101", "address the metric endpoint binds to")
	flag.StringVar(&probeAddr, "health-probe-bind-address", ":9102", "address the probe endpoint binds to")
	flag.BoolVar(&enableLeaderElection, "leader-elect", false, "enable leader election for HA controller managers")
	flag.StringVar(&watchNamespace, "namespace", os.Getenv("WATCH_NAMESPACE"), "namespace to watch; empty = all namespaces")
	opts := zap.Options{Development: false}
	opts.BindFlags(flag.CommandLine)
	flag.Parse()

	ctrl.SetLogger(zap.New(zap.UseFlagOptions(&opts)))

	if shutdown, terr := telemetry.Init(context.Background(), "dd-thread-operator"); terr != nil {
		fmt.Fprintf(os.Stderr, "telemetry init failed (continuing without traces): %v\n", terr)
	} else {
		defer func() { _ = shutdown(context.Background()) }()
	}

	cfg := ctrl.GetConfigOrDie()
	mgrOpts := ctrl.Options{
		Scheme:                 scheme,
		Metrics:                metricsserver.Options{BindAddress: metricsAddr},
		HealthProbeBindAddress: probeAddr,
		LeaderElection:         enableLeaderElection,
		LeaderElectionID:       "dd-thread-operator.dd.dev",
	}
	if watchNamespace != "" {
		// Restrict the cache (and therefore the controller) to a
		// single namespace. Useful for least-privilege deployments
		// where the operator only needs RoleBindings in one
		// namespace instead of cluster-wide permissions.
		mgrOpts.Cache = cache.Options{
			DefaultNamespaces: map[string]cache.Config{watchNamespace: {}},
		}
	}

	mgr, err := ctrl.NewManager(cfg, mgrOpts)
	if err != nil {
		fmt.Fprintf(os.Stderr, "manager init: %v\n", err)
		os.Exit(1)
	}

	if err := (&controller.ThreadReconciler{
		Client: mgr.GetClient(),
		Scheme: mgr.GetScheme(),
	}).SetupWithManager(mgr); err != nil {
		fmt.Fprintf(os.Stderr, "controller setup: %v\n", err)
		os.Exit(1)
	}

	if err := mgr.AddHealthzCheck("healthz", healthz.Ping); err != nil {
		fmt.Fprintf(os.Stderr, "healthz: %v\n", err)
		os.Exit(1)
	}
	if err := mgr.AddReadyzCheck("readyz", healthz.Ping); err != nil {
		fmt.Fprintf(os.Stderr, "readyz: %v\n", err)
		os.Exit(1)
	}

	if err := mgr.Start(ctrl.SetupSignalHandler()); err != nil {
		fmt.Fprintf(os.Stderr, "manager start: %v\n", err)
		os.Exit(1)
	}
}
