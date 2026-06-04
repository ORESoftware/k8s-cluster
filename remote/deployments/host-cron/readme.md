# Host cron deployment

Linux **systemd timers** ("cron") that live on the EC2 Kubernetes host, outside any
Kubernetes pod. Use this directory for jobs that:

- need access to host-level state that a containerised pod can't get safely
  (host PID, host containerd metadata, host process tables), or
- must keep running even when the runtime cluster is unhealthy (so they can't
  live in a Pod that may itself be the broken thing).

For anything that **can** live inside the cluster, prefer:

- a Kubernetes `CronJob` (e.g. `dd-headlamp-cron-sentinel.cronjob.yaml`), or
- a tokio loop in `idle-reaper-rs` (when the schedule is application-level and
  the work is HTTP/Kubernetes API only).

### Timers in this deployment

| Unit                                | Schedule           | What it does                                                                                                                                    |
| ----------------------------------- | ------------------ | ----------------------------------------------------------------------------------------------------------------------------------------------- |
| `dd-nerdctl-hook-reaper.timer`      | every 60 s         | Kill `nerdctl ... internal oci-hook postStop` procs older than `MAX_HOOK_AGE_SECONDS` (default 90 s), and `ctr -n dd-pool` tasks stuck in `UNKNOWN` for more than `MAX_UNKNOWN_AGE_SECONDS` (default 300 s). |
| `dd-pool-image-sync.timer`          | every 10 min       | Re-export `dd-dev-server:dev` from the `k8s.io` containerd namespace into `dd-pool` so newly-built worker images reach the warm-pool runtime without waiting for a reconcile.                                |

### `at` for one-off jobs

`at` is left available on the host for ad-hoc one-shot scheduling
(e.g. "rebuild the worker image at 04:00 tomorrow if we miss the regular
build"). We deliberately do **not** wire any `at` jobs as part of GitOps —
they're a manual operator escape hatch only. To schedule one:

```
echo '/usr/local/sbin/dd-nerdctl-hook-reaper.sh' | at now + 5 minutes
```

### How it's installed

`install.sh` is idempotent: it copies the files in `bin/` and `units/` to their
absolute paths on the host, runs `systemctl daemon-reload`, and enables every
timer in `units/*.timer`. The `reconcile-runtime` operation in
`.github/workflows/remote-k8s-maintenance.yml` invokes `install.sh` over SSM on
every push to `dev`, so the timers are reconciled from the repo on each deploy.

To install manually from inside the EC2 host:

```
sudo /home/ec2-user/codes/dd/dd-next-1/remote/deployments/host-cron/install.sh
```

To inspect runtime state:

```
systemctl list-timers --all 'dd-*.timer'
journalctl -u dd-nerdctl-hook-reaper.service --since '15 min ago'
journalctl -u dd-pool-image-sync.service --since '1 hour ago'
```
