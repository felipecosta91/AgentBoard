import { useEffect, useState } from 'react';
import QRCode from 'qrcode';
import { Check, Copy, ExternalLink } from 'lucide-react';
import { api, type State } from './api';
import { Dialog, ShimmerButton } from './ui';

type Tunnel = { ngrok_installed: boolean; ngrok_configured: boolean; cloudflared_installed: boolean; public_url: string };

const os = /Mac/i.test(navigator.userAgent) ? 'mac' : /Win/i.test(navigator.userAgent) ? 'win' : 'linux';
const INSTALL: Record<string, { label: string; cmd: string }> = {
  mac: { label: 'macOS (Homebrew)', cmd: 'brew install ngrok' },
  win: { label: 'Windows (PowerShell)', cmd: 'winget install ngrok.ngrok' },
  linux: { label: 'Linux (Snap)', cmd: 'sudo snap install ngrok' },
};

function Copy_({ text }: { text: string }) {
  const [done, setDone] = useState(false);
  return (
    <button aria-label="Copiar" className="shrink-0 text-neutral-400 hover:text-white"
      onClick={() => navigator.clipboard.writeText(text).then(() => { setDone(true); setTimeout(() => setDone(false), 1200); })}>
      {done ? <Check size={14} /> : <Copy size={14} />}
    </button>
  );
}

function Step({ n, done, title, children }: { n: number; done: boolean; title: string; children?: React.ReactNode }) {
  return (
    <section className="rounded-xl border border-line p-3">
      <h3 className="mb-2 flex items-center gap-2 text-sm font-semibold">
        <span className={`grid size-5 place-items-center rounded-full text-[11px] ${done ? 'bg-accent text-black' : 'bg-line text-neutral-300'}`}>{done ? <Check size={12} /> : n}</span>
        {title}
      </h3>
      {children && <div className="space-y-2 text-xs text-neutral-300">{children}</div>}
    </section>
  );
}

const openLink = (url: string) => api('POST', '/open-url', { url }).catch(() => window.open(url, '_blank'));

export function Phone({ settings, onClose }: { settings: State['settings']; onClose: () => void }) {
  const [tunnel, setTunnel] = useState<Tunnel>();
  const [authtoken, setAuthtoken] = useState('');
  const [err, setErr] = useState('');
  const [busy, setBusy] = useState(false);
  const [qr, setQr] = useState('');

  useEffect(() => {
    const load = () => api<Tunnel>('GET', '/tunnel').then(setTunnel).catch(() => {});
    load();
    const t = setInterval(load, 2500);
    return () => clearInterval(t);
  }, []);
  useEffect(() => { QRCode.toDataURL(settings.pair, { margin: 1, width: 240 }).then(setQr); }, [settings.pair]);

  const saveToken = async () => {
    setBusy(true);
    setErr('');
    try { setTunnel(await api<Tunnel>('POST', '/tunnel/ngrok', { authtoken: authtoken.trim() })); setAuthtoken(''); } catch (e) { setErr((e as Error).message); }
    setBusy(false);
  };

  const online = !!tunnel?.public_url;
  const install = INSTALL[os];
  const ready = tunnel?.ngrok_installed && tunnel.ngrok_configured;

  return (
    <Dialog title="Conectar o celular" onClose={onClose}>
      <div className="max-h-[75vh] space-y-3 overflow-y-auto pr-1">
        <p className="text-xs text-neutral-400">
          O app do celular acessa este computador pela internet por um túnel seguro do <b>ngrok</b> (grátis). Siga os passos — leva 2 minutos.
        </p>

        <Step n={1} done={!!tunnel?.ngrok_installed} title="Instale o ngrok">
          {!tunnel?.ngrok_installed && (
            <>
              <p>{install.label}:</p>
              <div className="flex items-center gap-2 rounded-lg bg-card p-2 font-mono"><code className="flex-1 break-all">{install.cmd}</code><Copy_ text={install.cmd} /></div>
              <button className="flex items-center gap-1 text-accent" onClick={() => openLink('https://ngrok.com/download')}>Outras opções de instalação <ExternalLink size={12} /></button>
            </>
          )}
        </Step>

        <Step n={2} done={!!ready} title="Crie a conta grátis e cole o authtoken">
          {tunnel?.ngrok_installed && !tunnel.ngrok_configured && (
            <>
              <p>
                <button className="text-accent underline" onClick={() => openLink('https://dashboard.ngrok.com/signup')}>Crie sua conta</button>, depois abra a página do{' '}
                <button className="text-accent underline" onClick={() => openLink('https://dashboard.ngrok.com/get-started/your-authtoken')}>authtoken</button> e copie o código.
              </p>
              <div className="flex gap-2">
                <input value={authtoken} onChange={(e) => setAuthtoken(e.target.value)} placeholder="Cole o authtoken aqui" type="password" autoComplete="off"
                  className="w-full rounded-lg border border-line bg-card px-3 py-2 outline-none focus:border-accent" />
                <ShimmerButton disabled={busy || authtoken.trim().length < 20} onClick={saveToken}>Salvar</ShimmerButton>
              </div>
              <p className="text-neutral-500">O token é guardado pelo próprio ngrok no seu computador. O AgentBoard não o armazena nem o envia a ninguém.</p>
              {err && <p className="text-red-400">{err}</p>}
            </>
          )}
        </Step>

        <Step n={3} done={online} title="Túnel e pareamento">
          {!online && ready && <p>Iniciando o túnel… (alguns segundos)</p>}
          {!ready && !tunnel?.cloudflared_installed && <p className="text-neutral-500">Conclua os passos acima para liberar o acesso remoto.</p>}
          {!ready && tunnel?.cloudflared_installed && <p className="text-neutral-500">Sem o ngrok, o app usa o <code>cloudflared</code> que já está instalado (mais lento para conectar).</p>}
          {(online || ready) && (
            <>
              {qr && <img src={qr} alt="QR de pareamento" className="mx-auto size-48 rounded-lg bg-white p-1" />}
              <p>Instale o APK no celular, abra-o e toque em <b>Escanear QR</b>.</p>
              <button className="flex w-full items-center justify-center gap-2 rounded-lg border border-line px-3 py-2 hover:bg-card"
                onClick={() => navigator.clipboard.writeText(settings.pair)}><Copy size={13} /> Copiar código de pareamento</button>
              {online && <p className="break-all text-neutral-500">Online: {tunnel?.public_url}</p>}
            </>
          )}
        </Step>

        <p className="text-[11px] text-amber-400">O QR e o código dão acesso total ao app (incluindo executar agentes). Não compartilhe.</p>
      </div>
    </Dialog>
  );
}
