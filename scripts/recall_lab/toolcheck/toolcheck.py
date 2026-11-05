from inspect_ai import Task, task
from inspect_ai.dataset import Sample
from inspect_ai.solver import solver
from inspect_ai.tool import text_editor, bash
from inspect_ai.util import sandbox

@solver
def probe():
    async def solve(state, generate):
        net = await sandbox().exec(["python3", "-c", "import socket; socket.create_connection((\"1.1.1.1\", 443), timeout=3)"])
        await sandbox().write_file("/tmp/x.txt", "hello\n")
        ed = text_editor()
        out = await ed(command="view", path="/tmp/x.txt")
        out2 = await ed(command="str_replace", path="/tmp/x.txt", old_str="hello", new_str="bye")
        cat = await sandbox().exec(["cat", "/tmp/x.txt"])
        state.metadata["probe"] = {"net_blocked": not net.success, "view": str(out)[:200], "replace": str(out2)[:200], "after": cat.stdout}
        return state
    return solve

@task
def toolcheck():
    return Task(dataset=[Sample(input="x")], solver=probe(), sandbox=("docker", "compose.yaml"))
