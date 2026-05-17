# GCS - Golang Chat Server

This deploys `ORESoftware/chat.vibe` as `gcs` in the default namespace.

The EC2 deployment mounts `ORESoftware/chat.vibe` from the EC2 host path:

```txt
/home/ec2-user/codes/dd/chat.vibe
```

The pod builds the Go binary into an `emptyDir` at startup. This avoids needing
ECR credentials while the chat service image pipeline is being cleaned up.

The current chat server starts two listeners:

- REST API: `gcs.default.svc.cluster.local:3000`
- WebSocket API: `gcs.default.svc.cluster.local:3001`
- Public REST health: `http://54.91.17.58/gcs/health`
- Public WebSocket listener health: `http://54.91.17.58/gcs/ws-health`

Health checks use:

```txt
/chat/v1/health/3000
```

## Backing Services

The EC2 kustomize app includes small in-cluster backing services:

- `gcs-mongodb` on `27017`
- `gcs-rabbitmq` on `5672`, with management on `15672`
- `gcs-kafka` on `9092`, with a single KRaft controller/broker
- existing `dd-redis-cache` on `6379`

RabbitMQ and Kafka have persistent hostPath data as requested:

```txt
/var/lib/dd/gcs/rabbitmq
/var/lib/dd/gcs/kafka
```

MongoDB is also persistent:

```txt
/var/lib/dd/gcs/mongodb
```

## Public Gateway Paths

The EC2 gateway routes:

- `/gcs/health` -> REST health check
- `/gcs/ws-health` -> WebSocket listener health check
- `/gcs/api/...` -> REST API with `/gcs/api` rewritten to `/chat`
- `/gcs/ws/...` -> WebSocket service

## Deploy

Apply the Argo CD application:

```bash
kubectl apply -f remote/argocd/apps/gcs.application.yaml
```

Then watch the app:

```bash
argocd app get gcs
kubectl get pods -l app=gcs -n default
```

When the chat image pipeline is ready again, update
`remote/gcs/k8s/ec2/gcs.deployment.yaml` to the desired image tag and remove the
EC2 hostPath source mount.
