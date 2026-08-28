# DIGEST/CREDENTIAL/POLICY-GATED TEMPLATE: excluded from active Kustomizations.
apiVersion: apps/v1
kind: Deployment
metadata:
  name: gha-capacity-broker-streempilot
  namespace: arc-runners-streempilot
  annotations:
    dd.dev/activation-state: image-digest-apps-billing-and-parity-gated
    dd.dev/linear-issue: DEN-1549
spec:
  replicas: 1
  selector:
    matchLabels:
      app: gha-capacity-broker-streempilot
  template:
    metadata:
      labels:
        app: gha-capacity-broker-streempilot
    spec:
      automountServiceAccountToken: false
      securityContext:
        runAsNonRoot: true
        runAsUser: 10001
        runAsGroup: 10001
        seccompProfile:
          type: RuntimeDefault
      containers:
        - name: server
          image: ghcr.io/oresoftware/gha-capacity-broker@sha256:REPLACE_GHA_CAPACITY_BROKER_IMAGE_DIGEST
          imagePullPolicy: IfNotPresent
          env:
            - name: HOST
              value: 0.0.0.0
            - name: PORT
              value: "8117"
            - name: GHA_ORGANIZATION
              value: StreemPilot
            - name: GHA_MUTATION_ENABLED
              value: "false"
            - name: GHA_RECONCILE_INTERVAL_SECONDS
              value: "900"
            - name: GHA_ORG_POLICY_JSON
              valueFrom:
                configMapKeyRef:
                  name: gha-capacity-broker-policy
                  key: policy.json
            - name: GITHUB_MUTATION_APP_ID
              valueFrom:
                secretKeyRef:
                  name: streempilot-gha-capacity-broker
                  key: github_app_id
            - name: GITHUB_MUTATION_APP_INSTALLATION_ID
              valueFrom:
                secretKeyRef:
                  name: streempilot-gha-capacity-broker
                  key: github_app_installation_id
            - name: GITHUB_MUTATION_APP_PRIVATE_KEY_PATH
              value: /var/run/gha-mutation-app/github_app_private_key
            - name: GITHUB_BILLING_APP_ID
              valueFrom:
                secretKeyRef:
                  name: streempilot-gha-billing
                  key: github_app_id
            - name: GITHUB_BILLING_APP_INSTALLATION_ID
              valueFrom:
                secretKeyRef:
                  name: streempilot-gha-billing
                  key: github_app_installation_id
            - name: GITHUB_BILLING_APP_PRIVATE_KEY_PATH
              value: /var/run/gha-billing-app/github_app_private_key
            - name: SERVER_AUTH_SECRET
              valueFrom:
                secretKeyRef:
                  name: streempilot-gha-capacity-broker
                  key: server_auth_secret
          ports:
            - name: http
              containerPort: 8117
          resources:
            requests:
              cpu: 50m
              memory: 96Mi
            limits:
              cpu: 500m
              memory: 256Mi
          securityContext:
            runAsNonRoot: true
            runAsUser: 10001
            allowPrivilegeEscalation: false
            readOnlyRootFilesystem: true
            capabilities:
              drop: ["ALL"]
            seccompProfile:
              type: RuntimeDefault
          startupProbe:
            httpGet:
              path: /healthz
              port: http
            failureThreshold: 30
            periodSeconds: 5
          readinessProbe:
            httpGet:
              path: /readyz
              port: http
            periodSeconds: 10
          livenessProbe:
            httpGet:
              path: /healthz
              port: http
            periodSeconds: 30
          volumeMounts:
            - name: mutation-app-key
              mountPath: /var/run/gha-mutation-app
              readOnly: true
            - name: billing-app-key
              mountPath: /var/run/gha-billing-app
              readOnly: true
            - name: tmp
              mountPath: /tmp
      volumes:
        - name: mutation-app-key
          secret:
            secretName: streempilot-gha-capacity-broker
            items:
              - key: github_app_private_key
                path: github_app_private_key
                mode: 0400
        - name: billing-app-key
          secret:
            secretName: streempilot-gha-billing
            items:
              - key: github_app_private_key
                path: github_app_private_key
                mode: 0400
        - name: tmp
          emptyDir:
            sizeLimit: 64Mi
---
apiVersion: v1
kind: Service
metadata:
  name: gha-capacity-broker-streempilot
  namespace: arc-runners-streempilot
spec:
  selector:
    app: gha-capacity-broker-streempilot
  ports:
    - name: http
      port: 8117
      targetPort: http
