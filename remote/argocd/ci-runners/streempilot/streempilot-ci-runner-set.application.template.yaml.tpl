# INERT TEMPLATE: outside active cloud overlays until every REPLACE_* token,
# GitHub App, runner group, image digest, and parity gate is reconciled.
apiVersion: argoproj.io/v1alpha1
kind: Application
metadata:
  name: streempilot-ci-runner-set-template
  namespace: argocd
  annotations:
    dd.dev/activation-state: template-only
    dd.dev/linear-issue: DEN-1549
  finalizers:
    - resources-finalizer.argocd.argoproj.io
spec:
  project: default
  source:
    repoURL: ghcr.io/actions/actions-runner-controller-charts
    chart: gha-runner-scale-set
    targetRevision: 0.14.2
    helm:
      releaseName: streempilot-ci-template
      skipTests: true
      values: |
        githubConfigUrl: https://github.com/StreemPilot
        githubConfigSecret: streempilot-arc-github
        runnerGroup: "REPLACE_RUNNER_GROUP"
        runnerScaleSetName: streempilot-ci
        minRunners: 0
        maxRunners: 2

        controllerServiceAccount:
          namespace: arc-systems
          name: streempilot-ci-arc-gha-rs-controller

        containerMode:
          type: ""

        listenerTemplate:
          spec:
            containers:
              - name: listener
                resources:
                  requests:
                    cpu: 100m
                    memory: 128Mi
                  limits:
                    cpu: 500m
                    memory: 512Mi
                securityContext:
                  runAsNonRoot: true
                  allowPrivilegeEscalation: false
                  capabilities:
                    drop: ["ALL"]
                  seccompProfile:
                    type: RuntimeDefault

        template:
          metadata:
            labels:
              dd.dev/ci-runner: streempilot-ci
              dd.dev/cloud-provider: REPLACE_CLOUD_PROVIDER
          spec:
            automountServiceAccountToken: false
            restartPolicy: Never
            terminationGracePeriodSeconds: 30
            securityContext:
              fsGroup: 1001
              runAsNonRoot: true
              runAsUser: 1001
              seccompProfile:
                type: RuntimeDefault
            containers:
              - name: runner
                image: ghcr.io/actions/actions-runner@sha256:REPLACE_ACTIONS_RUNNER_IMAGE_DIGEST
                imagePullPolicy: IfNotPresent
                command: ["/home/runner/run.sh"]
                env:
                  - name: ACTIONS_RUNNER_REQUIRE_JOB_CONTAINER
                    value: "false"
                resources:
                  requests:
                    cpu: "1"
                    memory: 2Gi
                    ephemeral-storage: 8Gi
                  limits:
                    cpu: "4"
                    memory: 8Gi
                    ephemeral-storage: 24Gi
                securityContext:
                  runAsNonRoot: true
                  runAsUser: 1001
                  allowPrivilegeEscalation: false
                  capabilities:
                    drop: ["ALL"]
                  seccompProfile:
                    type: RuntimeDefault
                volumeMounts:
                  - name: work
                    mountPath: /home/runner/_work
                  - name: tmp
                    mountPath: /tmp
            volumes:
              - name: work
                emptyDir:
                  sizeLimit: 20Gi
              - name: tmp
                emptyDir:
                  sizeLimit: 4Gi
  destination:
    server: https://kubernetes.default.svc
    namespace: arc-runners-streempilot
  syncPolicy:
    syncOptions:
      - CreateNamespace=true
      - ServerSideApply=true
