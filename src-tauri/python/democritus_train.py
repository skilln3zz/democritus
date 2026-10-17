"""Democritus build worker (Step 6E): QLoRA training of the student model.

Started by the app (build.rs), never by hand in normal use. It works offline
(HF_HUB_OFFLINE=1): the base model and lessons are local files.

  check                     environment report for Preflight (JSON)
  train --base --lessons --out --steps [--resume]
                            trains a LoRA adapter; progress as JSON lines on stdout
  merge --base --adapter --out
                            merges the adapter into the base weights for packaging
  convert --llama --model --outfile [--outtype q8_0]
                            runs llama.cpp's convert_hf_to_gguf.py. The app runs
                            Python isolated (-I), so the converter's own folder and
                            gguf-py are put on the import path here, and nothing else.

While training it reads <out>/control.json before every step:
{"action": "run" | "pause" | "cancel"}. Pause saves a checkpoint and exits with
code 3 (the GPU is free again); cancel exits with code 4; done exits with 0.
"""

import argparse
import json
import math
import os
import random
import sys
import time

os.environ.setdefault("HF_HUB_OFFLINE", "1")
os.environ.setdefault("TRANSFORMERS_OFFLINE", "1")
os.environ.setdefault("HF_HUB_DISABLE_TELEMETRY", "1")
os.environ.setdefault("PYTORCH_CUDA_ALLOC_CONF", "expandable_segments:True")

EXIT_PAUSED, EXIT_CANCELLED = 3, 4
# LoRA goes on the language model's projections only; the vision tower is left as it is.
LORA_TARGETS = r".*language_model.*\.(q_proj|k_proj|v_proj|o_proj|gate_proj|up_proj|down_proj|in_proj_qkvz|in_proj_ba|in_proj_qkv|in_proj_z|in_proj_b|in_proj_a|out_proj)$"


def emit(event, **fields):
    print(json.dumps({"event": event, **fields}), flush=True)


def check():
    report = {"python": sys.version.split()[0]}
    try:
        import torch
        report["torch"] = torch.__version__
        report["cuda"] = torch.version.cuda
        report["cudaAvailable"] = torch.cuda.is_available()
        if torch.cuda.is_available():
            report["gpu"] = torch.cuda.get_device_name(0)
            free, total = torch.cuda.mem_get_info()
            report["vramFree"], report["vramTotal"] = free, total
        import transformers, peft, bitsandbytes, accelerate  # noqa: F401
        import transformers.models as tm
        report["transformers"] = transformers.__version__
        report["peft"] = peft.__version__
        report["bitsandbytes"] = bitsandbytes.__version__
        report["qwen35"] = hasattr(tm, "qwen3_5")
        report["ok"] = bool(report["cudaAvailable"] and report["qwen35"])
    except Exception as e:  # report, don't crash: Preflight shows it
        report["ok"] = False
        report["error"] = f"{type(e).__name__}: {e}"
    print(json.dumps(report), flush=True)


def read_control(path):
    try:
        with open(path, encoding="utf-8") as f:
            return json.load(f).get("action", "run")
    except (OSError, ValueError):
        return "run"


def load_lessons(path, tokenizer, max_len):
    """Tokenizes each lesson: the prompt (everything before the last answer)
    is masked out, so only the answer is learned."""
    examples = []
    with open(path, encoding="utf-8") as f:
        for line in f:
            line = line.strip()
            if not line:
                continue
            msgs = json.loads(line)["messages"]
            if len(msgs) < 2 or msgs[-1]["role"] != "assistant":
                continue
            prompt = tokenizer.apply_chat_template(msgs[:-1], tokenize=False, add_generation_prompt=True, enable_thinking=False)
            answer = msgs[-1]["content"].strip() + "<|im_end|>\n"
            p = tokenizer(prompt, add_special_tokens=False)["input_ids"]
            a = tokenizer(answer, add_special_tokens=False)["input_ids"]
            if len(a) >= max_len:
                a = a[: max_len // 2]
            p = p[-(max_len - len(a)):]  # keep the end of a long prompt
            examples.append({"input_ids": p + a, "labels": [-100] * len(p) + a})
    return examples


def lr_at(step, total, base_lr, warmup):
    if step < warmup:
        return base_lr * (step + 1) / warmup
    progress = (step - warmup) / max(1, total - warmup)
    return base_lr * 0.5 * (1 + math.cos(math.pi * min(1.0, progress)))


def train(a):
    import torch
    from peft import LoraConfig, get_peft_model, prepare_model_for_kbit_training, set_peft_model_state_dict
    from transformers import AutoModelForImageTextToText, AutoTokenizer, BitsAndBytesConfig
    import bitsandbytes as bnb

    os.makedirs(a.out, exist_ok=True)
    control = os.path.join(a.out, "control.json")
    ckpt_dir = os.path.join(a.out, "checkpoint")
    random.seed(a.seed)
    torch.manual_seed(a.seed)

    emit("stage", text="Loading the student model in 4-bit")
    tok = AutoTokenizer.from_pretrained(a.base)
    bnb_cfg = BitsAndBytesConfig(load_in_4bit=True, bnb_4bit_quant_type="nf4", bnb_4bit_compute_dtype=torch.bfloat16,
                                 bnb_4bit_use_double_quant=True, llm_int8_skip_modules=["visual", "lm_head"])
    model = AutoModelForImageTextToText.from_pretrained(a.base, quantization_config=bnb_cfg, dtype=torch.bfloat16, device_map={"": 0})
    model.config.use_cache = False
    model = prepare_model_for_kbit_training(model, use_gradient_checkpointing=True, gradient_checkpointing_kwargs={"use_reentrant": False})
    model = get_peft_model(model, LoraConfig(r=a.rank, lora_alpha=a.rank * 2, lora_dropout=0.05, target_modules=LORA_TARGETS, task_type="CAUSAL_LM"))
    trainable = sum(p.numel() for p in model.parameters() if p.requires_grad)

    data = load_lessons(a.lessons, tok, a.max_len)
    if not data:
        emit("error", text="No usable lessons.")
        return 1
    params = [p for p in model.parameters() if p.requires_grad]
    opt = bnb.optim.PagedAdamW8bit(params, lr=a.lr, weight_decay=0.0)
    step, order, cursor = 0, list(range(len(data))), 0
    random.shuffle(order)
    state_file = os.path.join(ckpt_dir, "trainer_state.pt")
    if a.resume and os.path.exists(state_file):
        st = torch.load(state_file, weights_only=False)
        from safetensors.torch import load_file
        set_peft_model_state_dict(model, load_file(os.path.join(ckpt_dir, "adapter_model.safetensors")))
        opt.load_state_dict(st["optimizer"])
        step, order, cursor = st["step"], st["order"], st["cursor"]
        random.setstate(st["random"])
        emit("resumed", step=step)

    def save_checkpoint():
        os.makedirs(ckpt_dir, exist_ok=True)
        model.save_pretrained(ckpt_dir)
        tmp = state_file + ".tmp"
        torch.save({"optimizer": opt.state_dict(), "step": step, "order": order, "cursor": cursor, "random": random.getstate()}, tmp)
        os.replace(tmp, state_file)
        emit("checkpoint", step=step)

    emit("started", lessons=len(data), steps=a.steps, trainable=trainable, rank=a.rank)
    model.train()
    warmup = max(1, a.steps // 20)
    torch.cuda.reset_peak_memory_stats()
    while step < a.steps:
        action = read_control(control)
        if action == "cancel":
            emit("cancelled", step=step)
            return EXIT_CANCELLED
        if action == "pause":
            save_checkpoint()
            emit("paused", step=step)
            return EXIT_PAUSED
        t0 = time.time()
        for g in opt.param_groups:
            g["lr"] = lr_at(step, a.steps, a.lr, warmup)
        total_loss, tokens = 0.0, 0
        for _ in range(a.grad_accum):
            if cursor >= len(order):
                random.shuffle(order)
                cursor = 0
            ex = data[order[cursor]]
            cursor += 1
            ids = torch.tensor([ex["input_ids"]], device="cuda")
            labels = torch.tensor([ex["labels"]], device="cuda")
            loss = model(input_ids=ids, labels=labels).loss / a.grad_accum
            loss.backward()
            total_loss += loss.item()
            tokens += ids.shape[1]
        torch.nn.utils.clip_grad_norm_(params, 1.0)
        opt.step()
        opt.zero_grad(set_to_none=True)
        step += 1
        emit("step", step=step, total=a.steps, loss=round(total_loss, 4), lr=opt.param_groups[0]["lr"],
             seconds=round(time.time() - t0, 2), tokens=tokens, vramPeak=torch.cuda.max_memory_allocated())
        if step % a.checkpoint_every == 0 and step < a.steps:
            save_checkpoint()
    model.save_pretrained(os.path.join(a.out, "adapter"))
    emit("done", step=step)
    return 0


def merge(a):
    import shutil
    import torch
    from peft import PeftModel
    from transformers import AutoModelForImageTextToText

    emit("stage", text="Merging the adapter into the base weights")
    model = AutoModelForImageTextToText.from_pretrained(a.base, dtype=torch.bfloat16, device_map={"": "cpu"}, low_cpu_mem_usage=True)
    model = PeftModel.from_pretrained(model, a.adapter).merge_and_unload()
    os.makedirs(a.out, exist_ok=True)
    model.save_pretrained(a.out, safe_serialization=True, max_shard_size="5GB")
    for name in os.listdir(a.base):
        if name.endswith((".json", ".jinja", ".txt")) and not name.startswith("model.safetensors") and name != "config.json" and not os.path.exists(os.path.join(a.out, name)):
            shutil.copy2(os.path.join(a.base, name), os.path.join(a.out, name))
    emit("done", out=a.out)
    return 0


def convert(a):
    import runpy
    script = os.path.join(a.llama, "convert_hf_to_gguf.py")
    sys.path[:0] = [a.llama, os.path.join(a.llama, "gguf-py")]
    # --no-mtp: leave out the multi-token-prediction layer, as Ollama's own
    # qwen3.5 models do (transformers doesn't keep it, so it isn't in the merge).
    sys.argv = [script, a.model, "--outtype", a.outtype, "--outfile", a.outfile, "--no-mtp"]
    emit("stage", text="Converting to GGUF")
    try:
        runpy.run_path(script, run_name="__main__")
    except SystemExit as e:
        if e.code not in (None, 0):
            emit("error", text=f"The converter exited with {e.code}")
            return 1
    emit("done", out=a.outfile)
    return 0


def main():
    ap = argparse.ArgumentParser()
    sub = ap.add_subparsers(dest="cmd", required=True)
    sub.add_parser("check")
    t = sub.add_parser("train")
    t.add_argument("--base", required=True)
    t.add_argument("--lessons", required=True)
    t.add_argument("--out", required=True)
    t.add_argument("--steps", type=int, required=True)
    t.add_argument("--rank", type=int, default=16)
    t.add_argument("--lr", type=float, default=2e-4)
    t.add_argument("--max-len", dest="max_len", type=int, default=2048)
    t.add_argument("--grad-accum", dest="grad_accum", type=int, default=8)
    t.add_argument("--checkpoint-every", dest="checkpoint_every", type=int, default=25)
    t.add_argument("--seed", type=int, default=1234)
    t.add_argument("--resume", action="store_true")
    m = sub.add_parser("merge")
    m.add_argument("--base", required=True)
    m.add_argument("--adapter", required=True)
    m.add_argument("--out", required=True)
    c = sub.add_parser("convert")
    c.add_argument("--llama", required=True)
    c.add_argument("--model", required=True)
    c.add_argument("--outfile", required=True)
    c.add_argument("--outtype", default="q8_0", choices=["q8_0", "f16", "bf16"])
    a = ap.parse_args()
    try:
        if a.cmd == "check":
            check()
            return 0
        return {"train": train, "merge": merge, "convert": convert}[a.cmd](a)
    except Exception as e:
        import traceback
        emit("error", text=f"{type(e).__name__}: {e}", trace=traceback.format_exc()[-4000:])
        return 1


if __name__ == "__main__":
    sys.exit(main())
