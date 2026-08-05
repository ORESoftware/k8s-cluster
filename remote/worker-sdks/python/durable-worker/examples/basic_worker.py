from __future__ import annotations

import os

from oresoftware_durable_worker import DurableWorkerClient, TaskContext, WorkerConfig, run_worker


def echo(context: TaskContext):
    context.emit("accepted")
    context.raise_if_cancelled()
    return {"echo": dict(context.input), "fencingToken": context.fencing_token}


if __name__ == "__main__":
    client = DurableWorkerClient(
        os.environ.get("DURABLE_WORKER_URL", "http://127.0.0.1:8152"),
        os.environ["DURABLE_WORKER_AUTH_SECRET"],
    )
    print(
        run_worker(
            client,
            {"example.echo": echo},
            WorkerConfig(
                worker_id=os.environ.get("WORKER_ID", "python-example-worker"),
                queues=["examples"],
                capabilities=["python"],
                slots=2,
            ),
        )
    )
