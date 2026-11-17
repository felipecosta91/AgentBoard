import { useEffect, useState, type ReactNode } from 'react';
import { X } from 'lucide-react';

export function ShimmerButton({ children, ...p }: React.ButtonHTMLAttributes<HTMLButtonElement>) {
  return (
    <button
      {...p}
      className="shimmer inline-flex shrink-0 items-center gap-2 whitespace-nowrap rounded-xl bg-accent px-4 py-2 text-sm font-semibold text-black transition active:scale-95"
    >
      {children}
    </button>
  );
}

export function Dialog({ title, onClose, children }: { title: string; onClose: () => void; children: ReactNode }) {
  return (
    <div className="fixed inset-0 z-50 flex items-end justify-center bg-black/60 p-0 sm:items-center sm:p-4" onClick={onClose}>
      <div className="w-full max-w-md rounded-t-2xl border border-line bg-panel p-5 sm:rounded-2xl" onClick={(e) => e.stopPropagation()}>
        <div className="mb-4 flex items-center justify-between">
          <h2 className="font-semibold">{title}</h2>
          <button onClick={onClose} aria-label="Fechar"><X size={18} /></button>
        </div>
        {children}
      </div>
    </div>
  );
}

type Field = { name: string; label: string; area?: boolean; placeholder?: string };

export function FormDialog({ title, fields, submit, onClose, onSubmit }: {
  title: string; fields: Field[]; submit: string; onClose: () => void;
  onSubmit: (v: Record<string, string>) => Promise<unknown>;
}) {
  const [v, setV] = useState<Record<string, string>>({});
  const [err, setErr] = useState('');
  const [busy, setBusy] = useState(false);
  const cls = 'w-full rounded-lg border border-line bg-card px-3 py-2 text-sm outline-none focus:border-accent';
  return (
    <Dialog title={title} onClose={onClose}>
      <form
        className="space-y-3"
        onSubmit={async (e) => {
          e.preventDefault();
          setBusy(true);
          try { await onSubmit(v); onClose(); } catch (x) { setErr((x as Error).message); setBusy(false); }
        }}
      >
        {fields.map((f) => (
          <label key={f.name} className="block text-xs text-neutral-400">
            {f.label}
            {f.area
              ? <textarea className={cls + ' mt-1 h-24'} placeholder={f.placeholder} onChange={(e) => setV({ ...v, [f.name]: e.target.value })} />
              : <input className={cls + ' mt-1'} placeholder={f.placeholder} autoFocus={f === fields[0]} onChange={(e) => setV({ ...v, [f.name]: e.target.value })} />}
          </label>
        ))}
        {err && <p className="text-sm text-red-400">{err}</p>}
        <ShimmerButton disabled={busy} type="submit">{submit}</ShimmerButton>
      </form>
    </Dialog>
  );
}

// The native webview (WKWebView) silently ignores window.confirm/alert, so use in-app dialogs.
type Prompt = { message: string; confirm: boolean; resolve: (ok: boolean) => void };
let show: ((p: Prompt) => void) | undefined;

export const ask = (message: string) => new Promise<boolean>((resolve) => (show ? show({ message, confirm: true, resolve }) : resolve(false)));
export const tell = (message: string) => new Promise<boolean>((resolve) => (show ? show({ message, confirm: false, resolve }) : resolve(true)));

export function Prompts() {
  const [p, setP] = useState<Prompt>();
  useEffect(() => { show = setP; return () => { show = undefined; }; }, []);
  if (!p) return null;
  const close = (ok: boolean) => { p.resolve(ok); setP(undefined); };
  return (
    <Dialog title={p.confirm ? 'Confirmar' : 'Aviso'} onClose={() => close(false)}>
      <p className="mb-4 text-sm text-neutral-300">{p.message}</p>
      <div className="flex gap-2">
        <ShimmerButton onClick={() => close(true)} autoFocus>{p.confirm ? 'Confirmar' : 'OK'}</ShimmerButton>
        {p.confirm && <button onClick={() => close(false)} className="rounded-xl border border-line px-4 py-2 text-sm">Cancelar</button>}
      </div>
    </Dialog>
  );
}

const SPLASH_MIN_MS = 1500;

/** Fades out the splash declared in index.html once the app is ready (and it was visible long enough). */
export function hideSplash() {
  const el = document.getElementById('splash');
  if (!el || el.classList.contains('hide')) return;
  window.setTimeout(() => { el.classList.add('hide'); window.setTimeout(() => el.remove(), 600); }, Math.max(0, SPLASH_MIN_MS - performance.now()));
}
