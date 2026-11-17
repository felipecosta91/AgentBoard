import { useCallback, useEffect, useState } from 'react';
import { FolderKanban, KeyRound, Plus, Smartphone, Trash2 } from 'lucide-react';
import { api, getToken, setToken, wsUrl, type State } from './api';
import { Board } from './Board';
import { CardDrawer } from './CardDrawer';
import { FolderPicker } from './FolderPicker';
import { Phone } from './Phone';
import { ask, Dialog, hideSplash, Prompts, ShimmerButton } from './ui';

function useToken() {
  const [token, set] = useState(() => {
    const q = new URLSearchParams(location.search).get('token');
    if (q) {
      setToken(q);
      history.replaceState(null, '', location.pathname);
    }
    return getToken();
  });
  return [token, (t: string) => { setToken(t); set(t); }] as const;
}

export function App() {
  const [token, saveToken] = useToken();
  const [st, setSt] = useState<State>();
  const [tick, setTick] = useState(0);
  const [projectId, setProjectId] = useState(() => Number(localStorage.getItem('agentboard.project')) || 0);
  const [openCard, setOpenCard] = useState<number>();
  const [dialog, setDialog] = useState<'project' | 'phone' | 'accounts'>();
  const [error, setError] = useState('');

  const refresh = useCallback(async () => {
    try { setSt(await api<State>('GET', '/state')); setError(''); } catch (e) { setError((e as Error).message); }
    setTick((t) => t + 1);
  }, []);

  useEffect(() => {
    if (!token) return;
    refresh();
    let ws: WebSocket; let retry: number; let debounce: number; let closed = false;
    const connect = () => {
      ws = new WebSocket(wsUrl());
      ws.onmessage = () => { clearTimeout(debounce); debounce = window.setTimeout(refresh, 80); };
      ws.onclose = () => { if (!closed) retry = window.setTimeout(connect, 2000); };
    };
    connect();
    return () => { closed = true; clearTimeout(retry); clearTimeout(debounce); ws.close(); };
  }, [token, refresh]);

  const ready = !token || !!st || !!error;
  useEffect(() => {
    if (!st || localStorage.getItem('agentboard.onboarded')) return;
    localStorage.setItem('agentboard.onboarded', '1');
    setDialog('phone');
  }, [!!st]); // eslint-disable-line react-hooks/exhaustive-deps
  useEffect(() => { if (ready) hideSplash(); }, [ready]);

  if (!token) {
    return (
      <div className="grid h-full place-items-center p-6">
        <form className="w-full max-w-sm space-y-3" onSubmit={(e) => { e.preventDefault(); saveToken(String(new FormData(e.currentTarget).get('t') ?? '').trim()); }}>
          <h1 className="text-xl font-semibold">AgentBoard</h1>
          <input name="t" placeholder="Token de acesso" className="w-full rounded-lg border border-line bg-card px-3 py-2" autoFocus />
          <ShimmerButton type="submit">Entrar</ShimmerButton>
        </form>
      </div>
    );
  }

  const project = st?.projects.find((p) => p.id === projectId) ?? st?.projects[0];
  const select = (id: number) => { setProjectId(id); localStorage.setItem('agentboard.project', String(id)); };

  return (
    <div className="flex h-full flex-col md:flex-row">
      <aside className="flex shrink-0 gap-2 overflow-x-auto border-b border-line bg-panel p-3 md:w-60 md:flex-col md:overflow-visible md:border-b-0 md:border-r">
        <div className="hidden items-center gap-2 px-2 pb-3 md:flex">
          <div className="grid size-8 place-items-center rounded-lg bg-accent font-black text-black">A</div>
          <span className="font-semibold">AgentBoard</span>
        </div>
        <div className="hidden px-2 text-[11px] uppercase tracking-wider text-neutral-500 md:block">Projetos</div>
        {st?.projects.map((p) => (
          <div key={p.id} className="group flex shrink-0 items-center">
            <button onClick={() => select(p.id)} className={`flex flex-1 items-center gap-2 rounded-lg px-3 py-2 text-left text-sm ${p.id === project?.id ? 'bg-card text-accent' : 'text-neutral-300 hover:bg-card'}`}>
              <FolderKanban size={16} /> <span className="truncate">{p.name}</span>
            </button>
            <button aria-label="Remover projeto" className="hidden px-2 text-neutral-600 hover:text-red-400 group-hover:block"
              onClick={async () => { if (await ask(`Remover o projeto "${p.name}" e todos os cards?`)) { await api('DELETE', `/projects/${p.id}`); refresh(); } }}>
              <Trash2 size={14} />
            </button>
          </div>
        ))}
        <button onClick={() => setDialog('project')} className="flex shrink-0 items-center gap-2 rounded-lg px-3 py-2 text-sm text-neutral-400 hover:bg-card"><Plus size={16} /> Novo projeto</button>
        <button onClick={() => setDialog('accounts')} className="flex shrink-0 items-center gap-2 rounded-lg px-3 py-2 text-sm text-neutral-400 hover:bg-card md:mt-auto"><KeyRound size={16} /> Contas</button>
        <button onClick={() => setDialog('phone')} className="flex shrink-0 items-center gap-2 rounded-lg px-3 py-2 text-sm text-neutral-400 hover:bg-card"><Smartphone size={16} /> Celular</button>
      </aside>

      <main className="min-h-0 min-w-0 flex-1">
        {error && <div className="bg-red-950 px-4 py-2 text-sm text-red-300">{error}</div>}
        {st && project
          ? <Board st={st} project={project} setSt={setSt} refresh={refresh} openCard={setOpenCard} />
          : st && <div className="grid h-full place-items-center text-neutral-500">Crie um projeto para começar.</div>}
      </main>

      <Prompts />
      {openCard && st && <CardDrawer id={openCard} tick={tick} lanes={st.lanes} onClose={() => setOpenCard(undefined)} />}

      {dialog === 'project' && <NewProject onClose={() => setDialog(undefined)} onDone={refresh} />}
      {dialog === 'accounts' && <Accounts onClose={() => setDialog(undefined)} />}
      {dialog === 'phone' && st && <Phone settings={st.settings} onClose={() => setDialog(undefined)} />}
    </div>
  );
}

function NewProject({ onClose, onDone }: { onClose: () => void; onDone: () => Promise<void> }) {
  const [name, setName] = useState('');
  const [path, setPath] = useState('');
  const [picking, setPicking] = useState(false);
  const [err, setErr] = useState('');
  const cls = 'w-full rounded-lg border border-line bg-card px-3 py-2 text-sm outline-none focus:border-accent';
  const submit = async () => {
    try { await api('POST', '/projects', { name: name.trim() || path.split('/').pop(), path }); await onDone(); onClose(); } catch (e) { setErr((e as Error).message); }
  };
  return (
    <Dialog title="Novo projeto" onClose={onClose}>
      <div className="space-y-3">
        <label className="block text-xs text-neutral-400">Pasta do projeto
          <div className="mt-1 flex gap-2">
            <input value={path} onChange={(e) => setPath(e.target.value)} placeholder="/Users/voce/meu-projeto" className={cls} />
            <button onClick={() => setPicking(true)} className="shrink-0 rounded-lg border border-line px-3 text-sm hover:bg-card">Procurar…</button>
          </div>
        </label>
        <label className="block text-xs text-neutral-400">Nome (opcional)
          <input value={name} onChange={(e) => setName(e.target.value)} className={cls + ' mt-1'} />
        </label>
        {err && <p className="text-sm text-red-400">{err}</p>}
        <ShimmerButton disabled={!path} onClick={submit}>Criar</ShimmerButton>
      </div>
      {picking && <FolderPicker start={path} onPick={(p) => { setPath(p); setPicking(false); }} onClose={() => setPicking(false)} />}
    </Dialog>
  );
}

type Acct = { installed: boolean; logged_in: boolean; detail?: string };

function Accounts({ onClose }: { onClose: () => void }) {
  const [st, setSt] = useState<Record<string, Acct>>();
  const [err, setErr] = useState('');
  useEffect(() => {
    const load = () => api<Record<string, Acct>>('GET', '/auth').then(setSt).catch((e: Error) => setErr(e.message));
    load();
    const t = setInterval(load, 3000);
    return () => clearInterval(t);
  }, []);
  const login = (who: string) => api('POST', `/auth/${who}/login`).catch((e: Error) => setErr(e.message));
  return (
    <Dialog title="Contas dos agentes" onClose={onClose}>
      <div className="space-y-3">
        {(['claude', 'codex'] as const).map((who) => {
          const a = st?.[who];
          return (
            <div key={who} className="flex items-center gap-3 rounded-lg border border-line p-3">
              <span className={`size-2 rounded-full ${a?.logged_in ? 'bg-accent' : 'bg-red-400'}`} />
              <div className="flex-1 text-sm">
                <div className="font-medium capitalize">{who}</div>
                <div className="text-xs text-neutral-400">{!a ? '…' : !a.installed ? 'CLI não encontrada' : a.logged_in ? `Conectado ${a.detail ?? ''}` : 'Não conectado'}</div>
              </div>
              {a?.installed && !a.logged_in && <ShimmerButton onClick={() => login(who)}>Entrar</ShimmerButton>}
            </div>
          );
        })}
        <p className="text-xs text-neutral-500">"Entrar" abre o navegador do Mac para autorizar. O status atualiza sozinho.</p>
        {err && <p className="text-sm text-red-400">{err}</p>}
      </div>
    </Dialog>
  );
}

