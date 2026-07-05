# Deploying tor-server on Kubernetes

Deterministic bootstrap: relay identity keys are generated ahead of time so the
client directory (which lists each relay's public key) is known before anything
starts.

## 1. Build & push the image

```sh
docker build -t oresoftware/tor-server:0.1.0 .
docker push oresoftware/tor-server:0.1.0
```

## 2. Generate three relay keys

```sh
for r in a b c; do TOR_KEY_FILE=./relay-$r.key tor-server keygen; done
# note each printed `pubkey:` value
```

## 3. Create namespace, secrets, and directory

```sh
kubectl apply -f k8s/namespace.yaml

# relay secret keys (keeps raw keys out of git)
for r in a b c; do
  kubectl -n anon-proxy create secret generic tor-relay-$r-key \
    --from-file=relay.key=./relay-$r.key
done

# directory ConfigMap: copy the example, paste the three pubkeys, then apply
cp k8s/directory-configmap.example.yaml k8s/directory-configmap.yaml
$EDITOR k8s/directory-configmap.yaml
kubectl apply -f k8s/directory-configmap.yaml
```

## 4. Deploy relays and client

```sh
kubectl apply -f k8s/relays.yaml
kubectl apply -f k8s/client.yaml
```

## 5. Route traffic

Point any in-cluster workload at the SOCKS5 proxy:

```
socks5h://tor-client.anon-proxy.svc.cluster.local:9050
```

Files ending in `.example.yaml` are templates — copy, fill in, and apply the
non-example version (which is git-ignored via the repo `.gitignore` if you name
it `directory-configmap.yaml`). Never commit real key material.
