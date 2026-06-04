let input = "";
process.stdin.setEncoding("utf8");
process.stdin.on("data", (chunk) => {
  input += chunk;
});
process.stdin.on("end", () => {
  let request = {};
  try {
    request = input ? JSON.parse(input) : {};
  } catch (error) {
    request = { parseError: String(error), raw: input };
  }
  process.stdout.write(JSON.stringify({
    ok: true,
    runtime: "nodejs",
    pid: process.pid,
    request,
  }));
});
