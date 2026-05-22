// Hand-rolled deepcopy methods. Equivalent to what
// `controller-gen object` would produce. Re-run controller-gen when
// the spec gains new pointer/slice fields.
package v1alpha1

import (
	corev1 "k8s.io/api/core/v1"
	metav1 "k8s.io/apimachinery/pkg/apis/meta/v1"
	runtime "k8s.io/apimachinery/pkg/runtime"
)

func (in *Thread) DeepCopyInto(out *Thread) {
	*out = *in
	out.TypeMeta = in.TypeMeta
	in.ObjectMeta.DeepCopyInto(&out.ObjectMeta)
	in.Spec.DeepCopyInto(&out.Spec)
	in.Status.DeepCopyInto(&out.Status)
}

func (in *Thread) DeepCopy() *Thread {
	if in == nil {
		return nil
	}
	out := new(Thread)
	in.DeepCopyInto(out)
	return out
}

func (in *Thread) DeepCopyObject() runtime.Object {
	if c := in.DeepCopy(); c != nil {
		return c
	}
	return nil
}

func (in *ThreadList) DeepCopyInto(out *ThreadList) {
	*out = *in
	out.TypeMeta = in.TypeMeta
	in.ListMeta.DeepCopyInto(&out.ListMeta)
	if in.Items != nil {
		out.Items = make([]Thread, len(in.Items))
		for i := range in.Items {
			in.Items[i].DeepCopyInto(&out.Items[i])
		}
	}
}

func (in *ThreadList) DeepCopy() *ThreadList {
	if in == nil {
		return nil
	}
	out := new(ThreadList)
	in.DeepCopyInto(out)
	return out
}

func (in *ThreadList) DeepCopyObject() runtime.Object {
	if c := in.DeepCopy(); c != nil {
		return c
	}
	return nil
}

func (in *ThreadSpec) DeepCopyInto(out *ThreadSpec) {
	*out = *in
	if in.WorkspaceSize != nil {
		q := in.WorkspaceSize.DeepCopy()
		out.WorkspaceSize = &q
	}
	if in.StorageClassName != nil {
		s := *in.StorageClassName
		out.StorageClassName = &s
	}
	if in.Resources != nil {
		out.Resources = new(corev1.ResourceRequirements)
		in.Resources.DeepCopyInto(out.Resources)
	}
	if in.TTLSecondsAfterIdle != nil {
		v := *in.TTLSecondsAfterIdle
		out.TTLSecondsAfterIdle = &v
	}
	if in.LastActivityAt != nil {
		t := *in.LastActivityAt
		out.LastActivityAt = &t
	}
}

func (in *ThreadSpec) DeepCopy() *ThreadSpec {
	if in == nil {
		return nil
	}
	out := new(ThreadSpec)
	in.DeepCopyInto(out)
	return out
}

func (in *ThreadStatus) DeepCopyInto(out *ThreadStatus) {
	*out = *in
	if in.LastReconcileTime != nil {
		t := *in.LastReconcileTime
		out.LastReconcileTime = &t
	}
	if in.Conditions != nil {
		out.Conditions = make([]metav1.Condition, len(in.Conditions))
		for i := range in.Conditions {
			in.Conditions[i].DeepCopyInto(&out.Conditions[i])
		}
	}
}

func (in *ThreadStatus) DeepCopy() *ThreadStatus {
	if in == nil {
		return nil
	}
	out := new(ThreadStatus)
	in.DeepCopyInto(out)
	return out
}

