// Package controller implements the dd-thread-operator reconciler.
//
// Safety contract (v1alpha1, opt-in):
//
//   - The reconciler ONLY acts on Thread custom resources. The
//     existing template-based path
//     (remote/k8s/0[6-9]-thread-*.template.yaml + Vercel
//     `createK8sPod`) is unaffected because it does not produce
//     Thread CRs.
//   - Every child resource the operator creates carries the
//     dd.dev/managed-by=dd-thread-operator label. If the operator
//     finds an existing object with the same name but missing the
//     label, it REFUSES to mutate it and surfaces an
//     UnmanagedConflict condition. That makes accidental adoption of
//     a template-provisioned thread structurally impossible.
//   - Owner references (controller=true) are set on every child so
//     deletion of the Thread cascades correctly via Kubernetes
//     garbage collection.
package controller

import (
	"context"
	"fmt"
	"reflect"
	"time"

	appsv1 "k8s.io/api/apps/v1"
	corev1 "k8s.io/api/core/v1"
	networkingv1 "k8s.io/api/networking/v1"
	"k8s.io/apimachinery/pkg/api/equality"
	apierrors "k8s.io/apimachinery/pkg/api/errors"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	"k8s.io/apimachinery/pkg/runtime"
	"k8s.io/apimachinery/pkg/types"
	ctrl "sigs.k8s.io/controller-runtime"
	"sigs.k8s.io/controller-runtime/pkg/builder"
	"sigs.k8s.io/controller-runtime/pkg/client"
	"sigs.k8s.io/controller-runtime/pkg/controller/controllerutil"
	"sigs.k8s.io/controller-runtime/pkg/log"
	"sigs.k8s.io/controller-runtime/pkg/predicate"

	telemetry "github.com/oresoftware/dd/libs/telemetry-go"
	threadv1 "github.com/ORESoftware/k8s-cluster/remote/deployments/thread-operator-go/api/v1alpha1"
)

// defaultRequeue is how often we re-check a Thread to evaluate
// idle-timeout and TTL even when nothing has changed.
const defaultRequeue = 30 * time.Second

// ThreadReconciler reconciles Thread CRs into the matching set of
// child resources (PVC, Deployment, Service, Ingress).
type ThreadReconciler struct {
	client.Client
	Scheme *runtime.Scheme
}

func (r *ThreadReconciler) Reconcile(ctx context.Context, req ctrl.Request) (ctrl.Result, error) {
	ctx, span := telemetry.Tracer("dd-thread-operator").Start(ctx, "Reconcile "+req.NamespacedName.String())
	defer span.End()

	logger := log.FromContext(ctx).WithValues("thread", req.NamespacedName.String())

	var thread threadv1.Thread
	if err := r.Get(ctx, req.NamespacedName, &thread); err != nil {
		if apierrors.IsNotFound(err) {
			// Owner-ref garbage collection deletes children for us.
			return ctrl.Result{}, nil
		}
		return ctrl.Result{}, err
	}

	if !thread.DeletionTimestamp.IsZero() {
		// Cascade deletion is handled by k8s GC via owner refs.
		thread.Status.Phase = threadv1.ThreadPhaseTerminating
		_ = r.Status().Update(ctx, &thread)
		return ctrl.Result{}, nil
	}

	desiredReplicas := decideReplicas(&thread)

	// PVC --------------------------------------------------------
	pvcStatus := r.reconcilePVC(ctx, &thread)

	// Deployment ------------------------------------------------
	depStatus, observedDep := r.reconcileDeployment(ctx, &thread, desiredReplicas)

	// Service ----------------------------------------------------
	svcStatus := r.reconcileService(ctx, &thread)

	// Ingress ----------------------------------------------------
	ingStatus := r.reconcileIngress(ctx, &thread)

	// Status -----------------------------------------------------
	now := metav1.NewTime(time.Now().UTC())
	thread.Status.LastReconcileTime = &now
	thread.Status.ObservedGeneration = thread.Generation
	thread.Status.ReplicasDesired = desiredReplicas
	if observedDep != nil {
		thread.Status.ReplicasReady = observedDep.Status.ReadyReplicas
	}
	thread.Status.Phase = derivePhase(&thread, observedDep, desiredReplicas)
	r.fillPodObservation(ctx, &thread, observedDep)

	setCondition(&thread, threadv1.ConditionTypeWorkspaceProvisioned, pvcStatus)
	setCondition(&thread, threadv1.ConditionTypeDeploymentSynced, depStatus)
	setCondition(&thread, threadv1.ConditionTypeServiceSynced, svcStatus)
	setCondition(&thread, threadv1.ConditionTypeIngressSynced, ingStatus)
	setCondition(&thread, threadv1.ConditionTypeReady, aggregateReady(thread.Status.Phase, thread.Status.ReplicasReady, desiredReplicas))

	if err := r.Status().Update(ctx, &thread); err != nil && !apierrors.IsConflict(err) {
		logger.Error(err, "failed to update Thread status")
	}

	// TTL-based GC ----------------------------------------------
	if shouldDelete, after := evaluateTTL(&thread); shouldDelete {
		logger.Info("TTL elapsed; deleting Thread", "lastActivityAt", thread.Spec.LastActivityAt)
		if err := r.Delete(ctx, &thread); err != nil && !apierrors.IsNotFound(err) {
			return ctrl.Result{RequeueAfter: defaultRequeue}, err
		}
		return ctrl.Result{}, nil
	} else if after > 0 {
		// Requeue precisely when TTL would fire.
		return ctrl.Result{RequeueAfter: after}, nil
	}

	return ctrl.Result{RequeueAfter: defaultRequeue}, nil
}

func (r *ThreadReconciler) reconcilePVC(ctx context.Context, t *threadv1.Thread) reconcileOutcome {
	desired := BuildPVC(t)
	if err := controllerutil.SetControllerReference(t, desired, r.Scheme); err != nil {
		return failed("OwnerRefError", err.Error())
	}
	existing := &corev1.PersistentVolumeClaim{}
	err := r.Get(ctx, types.NamespacedName{Namespace: desired.Namespace, Name: desired.Name}, existing)
	switch {
	case apierrors.IsNotFound(err):
		if err := r.Create(ctx, desired); err != nil {
			return failed("CreateFailed", err.Error())
		}
		return ok("Created")
	case err != nil:
		return failed("GetFailed", err.Error())
	}
	if !HasManagedByLabel(existing.Labels) {
		return unmanaged(existing.Name)
	}
	// PVCs are essentially immutable once bound. Only update labels.
	if !reflect.DeepEqual(existing.Labels, desired.Labels) {
		patched := existing.DeepCopy()
		patched.Labels = desired.Labels
		if err := r.Update(ctx, patched); err != nil {
			return failed("UpdateFailed", err.Error())
		}
		return ok("LabelsUpdated")
	}
	return ok("Synced")
}

func (r *ThreadReconciler) reconcileDeployment(ctx context.Context, t *threadv1.Thread, replicas int32) (reconcileOutcome, *appsv1.Deployment) {
	desired := BuildDeployment(t, replicas)
	if err := controllerutil.SetControllerReference(t, desired, r.Scheme); err != nil {
		return failed("OwnerRefError", err.Error()), nil
	}
	existing := &appsv1.Deployment{}
	err := r.Get(ctx, types.NamespacedName{Namespace: desired.Namespace, Name: desired.Name}, existing)
	switch {
	case apierrors.IsNotFound(err):
		if err := r.Create(ctx, desired); err != nil {
			return failed("CreateFailed", err.Error()), nil
		}
		return ok("Created"), desired
	case err != nil:
		return failed("GetFailed", err.Error()), nil
	}
	if !HasManagedByLabel(existing.Labels) {
		return unmanaged(existing.Name), existing
	}
	patched := existing.DeepCopy()
	patched.Spec = desired.Spec
	patched.Labels = desired.Labels
	patched.OwnerReferences = desired.OwnerReferences
	// Skip the Update when nothing the operator owns has changed.
	// Without this, every periodic requeue (defaultRequeue) would
	// fire an Update against the API server even though desired
	// already matches existing, generating churn and audit-log
	// noise. Use semantic equality so the apiserver's defaulted
	// fields on existing don't trigger spurious diffs.
	if equality.Semantic.DeepEqual(existing.Spec, patched.Spec) &&
		equality.Semantic.DeepEqual(existing.Labels, patched.Labels) &&
		equality.Semantic.DeepEqual(existing.OwnerReferences, patched.OwnerReferences) {
		return ok("InSync"), existing
	}
	if err := r.Update(ctx, patched); err != nil {
		return failed("UpdateFailed", err.Error()), existing
	}
	return ok("Synced"), patched
}

func (r *ThreadReconciler) reconcileService(ctx context.Context, t *threadv1.Thread) reconcileOutcome {
	desired := BuildService(t)
	if err := controllerutil.SetControllerReference(t, desired, r.Scheme); err != nil {
		return failed("OwnerRefError", err.Error())
	}
	existing := &corev1.Service{}
	err := r.Get(ctx, types.NamespacedName{Namespace: desired.Namespace, Name: desired.Name}, existing)
	switch {
	case apierrors.IsNotFound(err):
		if err := r.Create(ctx, desired); err != nil {
			return failed("CreateFailed", err.Error())
		}
		return ok("Created")
	case err != nil:
		return failed("GetFailed", err.Error())
	}
	if !HasManagedByLabel(existing.Labels) {
		return unmanaged(existing.Name)
	}
	patched := existing.DeepCopy()
	patched.Labels = desired.Labels
	patched.OwnerReferences = desired.OwnerReferences
	patched.Spec.Selector = desired.Spec.Selector
	patched.Spec.Ports = desired.Spec.Ports
	patched.Spec.Type = desired.Spec.Type
	if equality.Semantic.DeepEqual(existing.Spec, patched.Spec) &&
		equality.Semantic.DeepEqual(existing.Labels, patched.Labels) &&
		equality.Semantic.DeepEqual(existing.OwnerReferences, patched.OwnerReferences) {
		return ok("InSync")
	}
	if err := r.Update(ctx, patched); err != nil {
		return failed("UpdateFailed", err.Error())
	}
	return ok("Synced")
}

func (r *ThreadReconciler) reconcileIngress(ctx context.Context, t *threadv1.Thread) reconcileOutcome {
	desired := BuildIngress(t)
	if err := controllerutil.SetControllerReference(t, desired, r.Scheme); err != nil {
		return failed("OwnerRefError", err.Error())
	}
	existing := &networkingv1.Ingress{}
	err := r.Get(ctx, types.NamespacedName{Namespace: desired.Namespace, Name: desired.Name}, existing)
	switch {
	case apierrors.IsNotFound(err):
		if err := r.Create(ctx, desired); err != nil {
			return failed("CreateFailed", err.Error())
		}
		return ok("Created")
	case err != nil:
		return failed("GetFailed", err.Error())
	}
	if !HasManagedByLabel(existing.Labels) {
		return unmanaged(existing.Name)
	}
	patched := existing.DeepCopy()
	patched.Labels = desired.Labels
	patched.Annotations = mergeStringMaps(existing.Annotations, desired.Annotations)
	patched.OwnerReferences = desired.OwnerReferences
	patched.Spec = desired.Spec
	if equality.Semantic.DeepEqual(existing.Spec, patched.Spec) &&
		equality.Semantic.DeepEqual(existing.Labels, patched.Labels) &&
		equality.Semantic.DeepEqual(existing.Annotations, patched.Annotations) &&
		equality.Semantic.DeepEqual(existing.OwnerReferences, patched.OwnerReferences) {
		return ok("InSync")
	}
	if err := r.Update(ctx, patched); err != nil {
		return failed("UpdateFailed", err.Error())
	}
	return ok("Synced")
}

func (r *ThreadReconciler) fillPodObservation(ctx context.Context, t *threadv1.Thread, dep *appsv1.Deployment) {
	if dep == nil {
		t.Status.PodIP = ""
		t.Status.PodName = ""
		t.Status.NodeName = ""
		return
	}
	pods := &corev1.PodList{}
	if err := r.List(ctx, pods, client.InNamespace(t.Namespace), client.MatchingLabels(SelectorLabels(t))); err != nil {
		return
	}
	for _, p := range pods.Items {
		if p.Status.Phase == corev1.PodRunning {
			t.Status.PodIP = p.Status.PodIP
			t.Status.PodName = p.Name
			t.Status.NodeName = p.Spec.NodeName
			return
		}
	}
	if len(pods.Items) > 0 {
		t.Status.PodName = pods.Items[0].Name
		t.Status.NodeName = pods.Items[0].Spec.NodeName
		t.Status.PodIP = pods.Items[0].Status.PodIP
	}
}

func decideReplicas(t *threadv1.Thread) int32 {
	if t.Spec.DesiredState == threadv1.ThreadDesiredStateSleeping {
		return 0
	}
	if t.Spec.IdleTimeoutSeconds > 0 && t.Spec.LastActivityAt != nil {
		idleFor := time.Since(t.Spec.LastActivityAt.Time)
		if idleFor > time.Duration(t.Spec.IdleTimeoutSeconds)*time.Second {
			return 0
		}
	}
	return 1
}

// evaluateTTL returns (delete?, requeueAfter). When TTLSecondsAfterIdle
// is unset, or when LastActivityAt has not yet been set, the operator
// has no time-based work to do and returns 0 so the caller falls back
// to defaultRequeue. Returning a long custom requeue here would make
// the operator skip its periodic idle/sleep checks.
func evaluateTTL(t *threadv1.Thread) (bool, time.Duration) {
	if t.Spec.TTLSecondsAfterIdle == nil {
		return false, 0
	}
	if t.Spec.LastActivityAt == nil {
		return false, 0
	}
	deadline := t.Spec.LastActivityAt.Time.Add(time.Duration(*t.Spec.TTLSecondsAfterIdle) * time.Second)
	now := time.Now()
	if !now.Before(deadline) {
		return true, 0
	}
	// Bound the custom requeue at defaultRequeue so we never silently
	// skip the periodic idle/replica check.
	until := deadline.Sub(now)
	if until > defaultRequeue {
		return false, defaultRequeue
	}
	return false, until
}

func derivePhase(t *threadv1.Thread, dep *appsv1.Deployment, desiredReplicas int32) threadv1.ThreadPhase {
	if !t.DeletionTimestamp.IsZero() {
		return threadv1.ThreadPhaseTerminating
	}
	if dep == nil {
		return threadv1.ThreadPhasePending
	}
	if desiredReplicas == 0 {
		return threadv1.ThreadPhaseSleeping
	}
	if dep.Status.UnavailableReplicas > 0 && dep.Status.ReadyReplicas == 0 {
		return threadv1.ThreadPhaseFailed
	}
	if dep.Status.ReadyReplicas >= desiredReplicas {
		return threadv1.ThreadPhaseActive
	}
	return threadv1.ThreadPhasePending
}

// reconcileOutcome is the per-step status the reconciler stamps onto
// Thread.Status.Conditions.
type reconcileOutcome struct {
	status  metav1.ConditionStatus
	reason  string
	message string
}

func ok(reason string) reconcileOutcome {
	return reconcileOutcome{status: metav1.ConditionTrue, reason: reason}
}

func failed(reason, message string) reconcileOutcome {
	return reconcileOutcome{status: metav1.ConditionFalse, reason: reason, message: message}
}

func unmanaged(name string) reconcileOutcome {
	return reconcileOutcome{
		status:  metav1.ConditionFalse,
		reason:  "UnmanagedConflict",
		message: fmt.Sprintf("object %q exists without dd.dev/managed-by=dd-thread-operator label; refusing to adopt", name),
	}
}

func aggregateReady(phase threadv1.ThreadPhase, ready, desired int32) reconcileOutcome {
	switch phase {
	case threadv1.ThreadPhaseActive:
		if ready >= desired {
			return ok("Active")
		}
	case threadv1.ThreadPhaseSleeping:
		return ok("Sleeping")
	}
	return reconcileOutcome{status: metav1.ConditionFalse, reason: string(phase)}
}

func setCondition(t *threadv1.Thread, condType string, outcome reconcileOutcome) {
	now := metav1.NewTime(time.Now().UTC())
	cond := metav1.Condition{
		Type:               condType,
		Status:             outcome.status,
		Reason:             stringOr(outcome.reason, "Unknown"),
		Message:            outcome.message,
		LastTransitionTime: now,
		ObservedGeneration: t.Generation,
	}
	for i, existing := range t.Status.Conditions {
		if existing.Type == condType {
			if existing.Status != cond.Status || existing.Reason != cond.Reason || existing.Message != cond.Message {
				t.Status.Conditions[i] = cond
			} else {
				// keep prior LastTransitionTime so it reflects real transitions
				t.Status.Conditions[i].ObservedGeneration = t.Generation
			}
			return
		}
	}
	t.Status.Conditions = append(t.Status.Conditions, cond)
}

func stringOr(a, b string) string {
	if a != "" {
		return a
	}
	return b
}

func mergeStringMaps(a, b map[string]string) map[string]string {
	out := make(map[string]string, len(a)+len(b))
	for k, v := range a {
		out[k] = v
	}
	for k, v := range b {
		out[k] = v
	}
	return out
}

// SetupWithManager wires the controller up to events on the Thread
// CR plus its owned children.
func (r *ThreadReconciler) SetupWithManager(mgr ctrl.Manager) error {
	managedOnly := predicate.NewPredicateFuncs(func(obj client.Object) bool {
		return HasManagedByLabel(obj.GetLabels())
	})
	return ctrl.NewControllerManagedBy(mgr).
		For(&threadv1.Thread{}).
		Owns(&corev1.PersistentVolumeClaim{}, builder.WithPredicates(managedOnly)).
		Owns(&appsv1.Deployment{}, builder.WithPredicates(managedOnly)).
		Owns(&corev1.Service{}, builder.WithPredicates(managedOnly)).
		Owns(&networkingv1.Ingress{}, builder.WithPredicates(managedOnly)).
		Complete(r)
}
