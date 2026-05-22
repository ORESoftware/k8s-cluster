// Package v1alpha1 contains the Thread CRD types for the dd-thread-operator.
//
// Group/Version: dd.dev/v1alpha1
// Kind:          Thread
//
// A Thread custom resource declares the desired lifecycle of one
// per-thread workspace pod. The operator reconciles a Thread CR into:
//
//   - a per-thread PersistentVolumeClaim (workspace),
//   - a per-thread Deployment (one Pod per thread, replicas=0|1),
//   - a per-thread ClusterIP Service,
//   - a per-thread Ingress (path-based per-thread routing).
//
// All child resources carry an OwnerReference back to the Thread CR
// and the label dd.dev/managed-by=dd-thread-operator. The operator
// REFUSES to mutate child resources that lack that label so it can
// never accidentally adopt or fight resources provisioned by the
// existing template-based path in remote/k8s/0[6-9]-thread-*.template.yaml.
package v1alpha1

import (
	"k8s.io/apimachinery/pkg/runtime/schema"
	"sigs.k8s.io/controller-runtime/pkg/scheme"
)

// GroupVersion is the group version used to register the Thread CRD.
var GroupVersion = schema.GroupVersion{Group: "dd.dev", Version: "v1alpha1"}

// SchemeBuilder is used to add go types to the GroupVersionKind scheme.
var SchemeBuilder = &scheme.Builder{GroupVersion: GroupVersion}

// AddToScheme adds the types in this group-version to the given scheme.
var AddToScheme = SchemeBuilder.AddToScheme
