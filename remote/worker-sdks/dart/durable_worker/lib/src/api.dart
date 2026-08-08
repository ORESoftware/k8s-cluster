import 'models.dart';

abstract interface class WorkerApi {
  Future<void> registerWorker(WorkerRegistration registration);

  Future<void> heartbeatWorker(String workerId, {bool? drain});

  Future<WorkerPoll> pollWorker(String workerId, {required Duration wait});

  Future<void> startStep(String stepId, Lease lease);

  Future<void> heartbeatStep(String stepId, Lease lease);

  Future<void> appendStepOutput(String stepId, StepOutput output);

  Future<void> completeStep(String stepId, StepCompletion completion);

  Future<void> failStep(String stepId, StepFailure failure);
}
