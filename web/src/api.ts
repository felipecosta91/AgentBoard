export type Kind = 'free' | 'refine' | 'refine_done' | 'create' | 'qa' | 'done';
export type Project = { id: number; name: string; path: string; wiki_status: string; wiki_files: number };
export type Lane = { id: number; project_id: number; name: string; kind: Kind; pos: number };
export type Card = {
  id: number; project_id: number; lane_id: number; title: string; body: string;
  agent: string; model: string; status: string; pos: number; archived: number; total: number; done: number; files: number;
};
export type State = {
  projects: Project[]; lanes: Lane[]; cards: Card[];
  settings: { ntfy_server: string; ntfy_topic: string; public_url: string; pair: string };
};
export type Detail = {
  card: Card & { context: string; lane_kind: Kind; lane_name: string };
  messages: { id: number; role: string; content: string; ts: number }[];
  checklist: { id: number; text: string; done: number }[];
  attachments: { id: number; name: string; kind: string; text: string }[];
};

const KEY = 'agentboard.token';
export const getToken = () => localStorage.getItem(KEY) ?? '';
export const setToken = (t: string) => localStorage.setItem(KEY, t);

export async function api<T = unknown>(method: string, path: string, body?: unknown): Promise<T> {
  const isForm = body instanceof FormData;
  const res = await fetch(`/api${path}`, {
    method,
    headers: {
      Authorization: `Bearer ${getToken()}`,
      ...(body && !isForm ? { 'Content-Type': 'application/json' } : {}),
    },
    body: body === undefined ? undefined : isForm ? body : JSON.stringify(body),
  });
  if (!res.ok) {
    const err = await res.json().catch(() => ({ message: res.statusText }));
    throw new Error(err.message ?? res.statusText);
  }
  const text = await res.text();
  return (text ? JSON.parse(text) : undefined) as T;
}

export const fileUrl = (id: number) => `/files/${id}?token=${getToken()}`;
export const wsUrl = () =>
  `${location.protocol === 'https:' ? 'wss' : 'ws'}://${location.host}/ws?token=${getToken()}`;
