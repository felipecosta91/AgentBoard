import { useEffect, useState } from 'react';
import { ArrowUp, Folder, FolderPlus } from 'lucide-react';
import { api } from './api';
import { Dialog, ShimmerButton } from './ui';

type Listing = { path: string; parent: string | null; dirs: string[] };

export function FolderPicker({ start, onPick, onClose }: { start: string; onPick: (path: string) => void; onClose: () => void }) {
  const [ls, setLs] = useState<Listing>();
  const [err, setErr] = useState('');
  const [name, setName] = useState('');
  const open = (path: string) =>
    api<Listing>('GET', `/fs?path=${encodeURIComponent(path)}`).then((l) => { setLs(l); setErr(''); }).catch((e: Error) => setErr(e.message));

  useEffect(() => { open(start); }, []); // eslint-disable-line react-hooks/exhaustive-deps

  const create = async () => {
    if (!ls || !name.trim()) return;
    try {
      const r = await api<{ path: string }>('POST', '/fs/mkdir', { path: ls.path, name: name.trim() });
      setName('');
      await open(r.path);
    } catch (e) { setErr((e as Error).message); }
  };

  return (
    <Dialog title="Escolher pasta" onClose={onClose}>
      <div className="mb-2 flex items-center gap-2">
        <button disabled={!ls?.parent} onClick={() => ls?.parent && open(ls.parent)} aria-label="Pasta acima" className="rounded-lg border border-line p-2 disabled:opacity-30"><ArrowUp size={16} /></button>
        <code className="min-w-0 flex-1 truncate text-xs text-neutral-400" dir="rtl">{ls?.path}</code>
      </div>
      <ul className="h-64 overflow-y-auto rounded-lg border border-line">
        {ls?.dirs.map((d) => (
          <li key={d}>
            <button onClick={() => open(`${ls.path}/${d}`)} className="flex w-full items-center gap-2 px-3 py-2 text-left text-sm hover:bg-card"><Folder size={14} className="text-accent" />{d}</button>
          </li>
        ))}
        {ls?.dirs.length === 0 && <li className="p-3 text-sm text-neutral-500">Sem subpastas.</li>}
      </ul>
      <form className="mt-2 flex gap-2" onSubmit={(e) => { e.preventDefault(); create(); }}>
        <input value={name} onChange={(e) => setName(e.target.value)} placeholder="Nova pasta aqui" className="w-full rounded-lg border border-line bg-card px-3 py-2 text-sm outline-none focus:border-accent" />
        <button type="submit" aria-label="Criar pasta" className="rounded-lg border border-line px-3"><FolderPlus size={16} /></button>
      </form>
      {err && <p className="mt-2 text-sm text-red-400">{err}</p>}
      <div className="mt-3"><ShimmerButton onClick={() => ls && onPick(ls.path)}>Usar esta pasta</ShimmerButton></div>
    </Dialog>
  );
}
