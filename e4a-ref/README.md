# e4a-ref

Reference driver for the durable Pi 1.5.0 scenarios. Requires Node >= 22.19.

Install without running package scripts:

```
npm ci --ignore-scripts
```

Run one scenario, or all of them when the argument is omitted:

```
E4A_KEY_FILE=<path> node run.mjs [scenario]
```

`<path>` is a file that contains only the OpenRouter key. Scenarios: `qa`, `read`, `kill-stream`, `kill-tool`, `fork`, `abort`, `switch`, `powershell`.

The driver uses free models. A paid model is used only when `E4A_ALLOW_PAID=1`.
