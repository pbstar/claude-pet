// 模型管理窗逻辑：条目列表 + 表单 CRUD
// 切换不在本窗做——只在螃蟹右键菜单；删除需确认
import { invoke } from "@tauri-apps/api/core";

type ModelDto = {
  id: string | null;
  name: string;
  format: "anthropic" | "openai";
  baseUrl: string;
  token: string;
  model: string;
  supports1m: boolean;
  active: boolean;
};

const $ = <T extends HTMLElement>(sel: string): T => {
  const el = document.querySelector(sel);
  if (!el) throw new Error(`missing element: ${sel}`);
  return el as T;
};

const listEl = $<HTMLUListElement>("#list");
const form = $<HTMLFormElement>("#form");
const editingId = $<HTMLSpanElement>("#editing-tag");
const msgEl = $<HTMLDivElement>("#msg");

let editing: string | null = null; // null = 新增

async function refresh(): Promise<void> {
  const models = await invoke<ModelDto[]>("list_models").catch(() => []);
  listEl.textContent = "";
  if (models.length === 0) {
    const li = document.createElement("li");
    li.textContent = "暂无条目，用下方表单添加";
    listEl.appendChild(li);
    return;
  }
  for (const m of models) {
    const li = document.createElement("li");
    if (m.active) li.className = "active";

    const name = document.createElement("span");
    name.className = "name";
    name.textContent = m.name;
    li.appendChild(name);

    const meta = document.createElement("span");
    meta.className = "meta";
    meta.textContent = `${m.model}${m.supports1m ? " · 1M" : ""}`;
    li.appendChild(meta);

    const editBtn = document.createElement("button");
    editBtn.textContent = "编辑";
    editBtn.addEventListener("click", () => startEdit(m));
    li.appendChild(editBtn);

    const delBtn = document.createElement("button");
    delBtn.textContent = "删除";
    delBtn.className = "danger";
    delBtn.addEventListener("click", () => void remove(m));
    li.appendChild(delBtn);

    listEl.appendChild(li);
  }
}

function startEdit(m: ModelDto): void {
  editing = m.id;
  editingId.textContent = `（${m.name}）`;
  $<HTMLInputElement>("#name").value = m.name;
  $<HTMLSelectElement>("#format").value = m.format;
  $<HTMLInputElement>("#baseUrl").value = m.baseUrl;
  $<HTMLInputElement>("#token").value = m.token;
  $<HTMLInputElement>("#model").value = m.model;
  $<HTMLInputElement>("#supports1m").checked = m.supports1m;
  $<HTMLButtonElement>("#cancel").hidden = false;
  msgEl.textContent = "";
}

function resetForm(): void {
  editing = null;
  editingId.textContent = "";
  form.reset();
  $<HTMLButtonElement>("#cancel").hidden = true;
  msgEl.textContent = "";
}

$<HTMLButtonElement>("#cancel").addEventListener("click", resetForm);

// 测试连通性：用表单当前值（不要求已保存），结果复用 #msg 展示
$<HTMLButtonElement>("#test").addEventListener("click", (e) => {
  e.preventDefault(); // 防止在 form 内触发 submit
  void (async () => {
    const btn = e.currentTarget as HTMLButtonElement;
    btn.disabled = true;
    btn.textContent = "测试中…";
    msgEl.className = "";
    msgEl.textContent = "";
    const result = await invoke<string>("test_model", {
      format: $<HTMLSelectElement>("#format").value,
      baseUrl: $<HTMLInputElement>("#baseUrl").value,
      token: $<HTMLInputElement>("#token").value,
      model: $<HTMLInputElement>("#model").value,
    }).catch((err) => `失败：${String(err)}`);
    msgEl.className = result.startsWith("连通成功") ? "ok" : "";
    msgEl.textContent = result;
    btn.disabled = false;
    btn.textContent = "测试";
  })();
});

form.addEventListener("submit", (e) => {
  e.preventDefault();
  void (async () => {
    msgEl.textContent = "";
    const ok = await invoke("save_model", {
      id: editing,
      name: $<HTMLInputElement>("#name").value,
      format: $<HTMLSelectElement>("#format").value,
      baseUrl: $<HTMLInputElement>("#baseUrl").value,
      token: $<HTMLInputElement>("#token").value,
      model: $<HTMLInputElement>("#model").value,
      supports1m: $<HTMLInputElement>("#supports1m").checked,
    })
      .then(() => true)
      .catch((err) => {
        msgEl.textContent = String(err);
        return false;
      });
    if (ok) {
      resetForm();
      await refresh();
    }
  })();
});

async function remove(m: ModelDto): Promise<void> {
  const yes = window.confirm(`删除「${m.name}」？`);
  if (!yes) return;
  await invoke("delete_model", { id: m.id }).catch((err) => {
    msgEl.textContent = String(err);
  });
  if (editing === m.id) resetForm();
  await refresh();
}

void refresh();
