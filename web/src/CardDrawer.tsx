import { useEffect, useRef, useState } from 'react';
import Markdown from 'react-markdown';
import { Link2, Paperclip, Send, Square, Trash2, X } from 'lucide-react';
import { api, fileUrl, type Detail, type Lane } from './api';
import { ask } from './ui';

const input = 'w-full rounded-lg border border-line bg-card px-3 py-2 text-sm outline-none focus:border-accent';

export function CardDrawer({ id, tick, lanes, onClose }: { id: number; tick: number; lanes: Lane[]; onClose: () => void }) {
  const [d, setD] = useState<Detail>();
  const [text, setText] = useState('');
  const [err, setErr] = useState('');
  const [newItem, setNewItem] = useState('');
  const [url, setUrl] = useState('');
  const [uploading, setUploading] = useState(false);
  const [models, setModels] = useState<Record<string, string[]>>({});
  const bottom = useRef<HTMLDivElement>(null);

  useEffect(() => { api<Record<string, string[]>>('GET', '/models').then(setModels).catch(() => {}); }, []);

  useEffect(() => { api<Detail>('GET', `/cards/${id}`).then(setD).catch((e) => setErr(e.message)); }, [id, tick]);
  useEffect(() => { bottom.current?.scrollIntoView({ block: 'end' }); }, [d?.messages.length]);

  if (!d) return null;
  const { card } = d;
  const running = card.status === 'running';
  const act = (p: Promise<unknown>) => p.catch((e: Error) => setErr(e.message));
  const patch = (body: object) => act(api('PATCH', `/cards/${id}`, body));

  const send = async () => {
    const t = text.trim();
    if (!t || running) return;
    setText('');
    setErr('');
    await act(api('POST', `/cards/${id}/chat`, { text: t }));
  };
  const upload = async (files: FileList | null) => {
    if (!files?.length) return;
    const fd = new FormData();
    [...files].forEach((f) => fd.append('file', f));
    setUploading(true);
    await act(api('POST', `/cards/${id}/files`, fd));
    setUploading(false);
  };

  return (
    <div className="fixed inset-0 z-40 bg-black/60" onClick={onClose}>
      <div className="absolute inset-y-0 right-0 flex w-full flex-col border-l border-line bg-bg md:w-[min(1100px,94vw)]" onClick={(e) => e.stopPropagation()}>
        <header className="flex items-center gap-3 border-b border-line px-4 py-3">
          <input key={card.title} defaultValue={card.title} onBlur={(e) => e.target.value !== card.title && patch({ title: e.target.value })}
            className="min-w-0 flex-1 bg-transparent text-lg font-semibold outline-none" />
          <select aria-label="Mover para" value={card.lane_id} className="max-w-36 rounded bg-card px-2 py-1 text-xs text-neutral-300"
            onChange={(e) => act(api('POST', `/cards/${id}/move`, { lane_id: Number(e.target.value) }))}>
            {lanes.filter((l) => l.project_id === card.project_id).sort((a, b) => a.pos - b.pos).map((l) => <option key={l.id} value={l.id}>{l.name}</option>)}
          </select>
          <button onClick={onClose} aria-label="Fechar"><X size={20} /></button>
        </header>

        <div className="grid min-h-0 flex-1 md:grid-cols-[1fr_340px]">
          <section className="flex min-h-0 flex-col border-line md:border-r">
            <div className="flex-1 space-y-3 overflow-y-auto p-4">
              {d.messages.length === 0 && <p className="text-sm text-neutral-500">Sem mensagens. Mova o card para uma categoria de agente ou converse abaixo.</p>}
              {d.messages.map((m) =>
                m.role === 'user' ? (
                  <div key={m.id} className="ml-auto max-w-[85%] whitespace-pre-wrap rounded-2xl bg-card px-4 py-2 text-sm">{m.content}</div>
                ) : m.role === 'assistant' ? (
                  <div key={m.id} className="md max-w-[92%] text-sm leading-relaxed"><Markdown>{m.content}</Markdown></div>
                ) : (
                  <div key={m.id} className="truncate font-mono text-[11px] text-neutral-500">{m.role === 'tool' ? '🔧 ' : ''}{m.content}</div>
                ))}
              {running && <div className="beam w-fit rounded-full border border-line px-3 py-1 text-xs text-accent">agente trabalhando…</div>}
              <div ref={bottom} />
            </div>
            {err && <p className="px-4 pb-1 text-sm text-red-400">{err}</p>}
            <div className="flex items-end gap-2 border-t border-line p-3">
              <textarea value={text} onChange={(e) => setText(e.target.value)} rows={2} placeholder={running ? 'Aguarde o agente…' : 'Fale com o agente…'}
                onKeyDown={(e) => { if (e.key === 'Enter' && !e.shiftKey) { e.preventDefault(); send(); } }} className={input + ' resize-none'} />
              {running
                ? <button onClick={() => act(api('POST', `/cards/${id}/stop`))} className="rounded-lg bg-red-500/20 p-3 text-red-300" aria-label="Parar"><Square size={16} /></button>
                : <button onClick={send} className="rounded-lg bg-accent p-3 text-black" aria-label="Enviar"><Send size={16} /></button>}
            </div>
          </section>

          <aside className="min-h-0 space-y-5 overflow-y-auto p-4 text-sm">
            <div className="grid grid-cols-2 gap-2">
              <label className="text-xs text-neutral-400">Agente
                <select value={card.agent} onChange={(e) => patch({ agent: e.target.value, model: models[e.target.value]?.[0] ?? '' })} className={input + ' mt-1'}>
                  <option value="claude">Claude</option><option value="codex">Codex</option>
                </select>
              </label>
              <label className="text-xs text-neutral-400">Modelo
                <select value={card.model} onChange={(e) => patch({ model: e.target.value })} className={input + ' mt-1'}>
                  <option value="">Padrão da CLI</option>
                  {[...(models[card.agent] ?? []), ...(card.model && !(models[card.agent] ?? []).includes(card.model) ? [card.model] : [])].map((m) => <option key={m} value={m}>{m}</option>)}
                </select>
              </label>
            </div>

            <label className="block text-xs text-neutral-400">Descrição
              <textarea key={card.body} defaultValue={card.body} rows={4} className={input + ' mt-1'} onBlur={(e) => e.target.value !== card.body && patch({ body: e.target.value })} />
            </label>

            <div>
              <h4 className="mb-2 text-xs uppercase tracking-wider text-neutral-500">Checklist {d.checklist.filter((i) => i.done).length}/{d.checklist.length}</h4>
              <ul className="space-y-1">
                {d.checklist.map((i) => (
                  <li key={i.id} className="group flex items-start gap-2">
                    <input type="checkbox" checked={!!i.done} className="mt-1 accent-lime-400" onChange={(e) => act(api('PATCH', `/checklist/${i.id}`, { done: e.target.checked }))} />
                    <span className={`flex-1 ${i.done ? 'text-neutral-500 line-through' : ''}`}>{i.text}</span>
                    <button onClick={() => act(api('DELETE', `/checklist/${i.id}`))} className="hidden text-neutral-600 hover:text-red-400 group-hover:block" aria-label="Remover"><Trash2 size={13} /></button>
                  </li>
                ))}
              </ul>
              <form className="mt-2 flex gap-2" onSubmit={(e) => { e.preventDefault(); if (newItem.trim()) { act(api('POST', `/cards/${id}/checklist`, { text: newItem })); setNewItem(''); } }}>
                <input value={newItem} onChange={(e) => setNewItem(e.target.value)} placeholder="Novo item" className={input} />
              </form>
            </div>

            <label className="block text-xs text-neutral-400">Contexto (compartilhado com os agentes · RAG)
              <textarea key={card.context} defaultValue={card.context} rows={8} className={input + ' mt-1 font-mono text-xs'} onBlur={(e) => e.target.value !== card.context && patch({ context: e.target.value })} />
            </label>

            <div>
              <h4 className="mb-2 text-xs uppercase tracking-wider text-neutral-500">Anexos</h4>
              <ul className="space-y-2">
                {d.attachments.map((a) => (
                  <li key={a.id} className="rounded-lg border border-line p-2">
                    <div className="flex items-center gap-2">
                      {a.kind === 'image' && <img src={fileUrl(a.id)} alt="" className="size-10 rounded object-cover" />}
                      <span className="flex-1 truncate text-xs">{a.kind === 'audio' ? '🎙 ' : a.kind === 'url' ? '🔗 ' : ''}{a.name}</span>
                      <button onClick={() => act(api('DELETE', `/attachments/${a.id}`))} className="text-neutral-600 hover:text-red-400" aria-label="Remover"><Trash2 size={13} /></button>
                    </div>
                    {a.kind === 'audio' && <p className="mt-1 text-xs text-neutral-400">{a.text}</p>}
                  </li>
                ))}
              </ul>
              <label className="mt-2 flex cursor-pointer items-center gap-2 rounded-lg border border-dashed border-line px-3 py-2 text-xs text-neutral-400 hover:border-accent">
                <Paperclip size={14} /> {uploading ? 'Enviando…' : 'Texto, imagem ou áudio'}
                <input type="file" multiple hidden onChange={(e) => { upload(e.target.files); e.target.value = ''; }} />
              </label>
              <form className="mt-2 flex items-center gap-2" onSubmit={async (e) => { e.preventDefault(); if (url.trim()) { await act(api('POST', `/cards/${id}/url`, { url: url.trim() })); setUrl(''); } }}>
                <Link2 size={14} className="text-neutral-500" />
                <input value={url} onChange={(e) => setUrl(e.target.value)} placeholder="https://… (acesso web)" className={input} />
              </form>
            </div>

            <button onClick={async () => { await act(api('POST', `/cards/${id}/archive`, { archived: !card.archived })); if (!card.archived) onClose(); }} className="mr-4 text-xs text-neutral-300">{card.archived ? 'Restaurar card' : 'Arquivar card'}</button>
            <button onClick={async () => { if (await ask('Remover este card?')) { await act(api('DELETE', `/cards/${id}`)); onClose(); } }} className="text-xs text-red-400">Remover card</button>
          </aside>
        </div>
      </div>
    </div>
  );
}
