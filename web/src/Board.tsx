import { useEffect, useState } from 'react';
import { DndContext, DragOverlay, MouseSensor, TouchSensor, useDraggable, useDroppable, useSensor, useSensors, type DragEndEvent } from '@dnd-kit/core';
import { Archive, ArchiveRestore, ArrowRightLeft, CheckSquare, FolderOpen, Paperclip, Plus, SquareTerminal, Trash2, X } from 'lucide-react';
import { api, type Card, type Kind, type Lane, type Project, type State } from './api';
import { ask, Dialog, FormDialog, ShimmerButton, tell } from './ui';

const KIND: Record<Kind, { label: string; dot: string }> = {
  free: { label: 'Livre', dot: 'bg-neutral-500' },
  refine: { label: 'Refinar · agente', dot: 'bg-violet-400' },
  refine_done: { label: 'Aguarda você', dot: 'bg-sky-400' },
  create: { label: 'Criar · agente', dot: 'bg-accent' },
  qa: { label: 'Q.A · agente', dot: 'bg-amber-400' },
  done: { label: 'Concluído', dot: 'bg-emerald-400' },
};

function useIsMobile() {
  const query = '(max-width: 767px)';
  const [mobile, setMobile] = useState(() => window.matchMedia(query).matches);
  useEffect(() => {
    const mq = window.matchMedia(query);
    const on = () => setMobile(mq.matches);
    mq.addEventListener('change', on);
    return () => mq.removeEventListener('change', on);
  }, []);
  return mobile;
}

function CardView({ card, onOpen, ghost }: { card: Card; onOpen?: () => void; ghost?: boolean }) {
  const running = card.status === 'running';
  const pct = card.total ? (card.done / card.total) * 100 : 0;
  const badge = card.status === 'error' ? 'text-red-400' : card.status === 'failed' ? 'text-amber-400' : '';
  return (
    <div onClick={onOpen} className={`${running ? 'beam' : ''} cursor-pointer rounded-xl border border-line bg-card p-3 text-sm ${ghost ? 'rotate-2 shadow-2xl' : ''}`}>
      <div className="font-medium leading-snug">{card.title}</div>
      {card.body && <div className="mt-1 line-clamp-2 text-xs text-neutral-400">{card.body}</div>}
      {card.total > 0 && (
        <div className="mt-2 h-1 overflow-hidden rounded bg-line"><div className="h-full bg-accent transition-all" style={{ width: `${pct}%` }} /></div>
      )}
      <div className="mt-2 flex items-center gap-3 text-[11px] text-neutral-400">
        <span className="rounded bg-line px-1.5 py-0.5">{card.agent}{card.model ? ` · ${card.model}` : ''}</span>
        {card.total > 0 && <span className="flex items-center gap-1"><CheckSquare size={12} />{card.done}/{card.total}</span>}
        {card.files > 0 && <span className="flex items-center gap-1"><Paperclip size={12} />{card.files}</span>}
        {(running || badge) && <span className={`ml-auto ${badge || 'text-accent'}`}>{running ? 'trabalhando…' : card.status === 'error' ? 'erro' : 'reprovado'}</span>}
      </div>
    </div>
  );
}

function DraggableCard({ card, onOpen, onMenu }: { card: Card; onOpen: () => void; onMenu: (e: React.MouseEvent) => void }) {
  const drag = useDraggable({ id: `card:${card.id}` });
  const drop = useDroppable({ id: `card:${card.id}` });
  return (
    <div onContextMenu={onMenu} ref={(n) => { drag.setNodeRef(n); drop.setNodeRef(n); }} {...drag.listeners} {...drag.attributes}
      className={`touch-manipulation ${drag.isDragging ? 'opacity-30' : ''} ${drop.isOver && !drag.isDragging ? 'border-t-2 border-accent' : ''}`}>
      <CardView card={card} onOpen={onOpen} />
    </div>
  );
}

function LaneView({ lane, cards, onOpen, onMenu, onAdd, onDelete }: {
  lane: Lane; cards: Card[]; onOpen: (id: number) => void; onMenu: (e: React.MouseEvent, c: Card) => void; onAdd: () => void; onDelete: () => void;
}) {
  const { setNodeRef, isOver } = useDroppable({ id: `lane:${lane.id}` });
  const k = KIND[lane.kind];
  return (
    <section className="flex max-h-full w-72 shrink-0 flex-col rounded-2xl border border-line bg-panel">
      <header className="flex items-center gap-2 px-3 py-3">
        <span className={`size-2 rounded-full ${k.dot}`} />
        <h3 className="text-sm font-semibold">{lane.name}</h3>
        <span className="text-xs text-neutral-500">{cards.length}</span>
        <span className="ml-1 hidden text-[10px] text-neutral-500 sm:inline">{k.label}</span>
        <button onClick={onAdd} aria-label="Novo card" className="ml-auto text-neutral-400 hover:text-white"><Plus size={16} /></button>
        {lane.kind === 'free' && <button onClick={onDelete} aria-label="Remover categoria" className="text-neutral-600 hover:text-red-400"><X size={14} /></button>}
      </header>
      <div ref={setNodeRef} className={`flex min-h-24 flex-1 flex-col gap-2 overflow-y-auto px-2 pb-2 ${isOver ? 'bg-accent/5' : ''}`}>
        {cards.map((c) => <DraggableCard key={c.id} card={c} onOpen={() => onOpen(c.id)} onMenu={(e) => onMenu(e, c)} />)}
      </div>
    </section>
  );
}

export function Board({ st, project, setSt, refresh, openCard }: {
  st: State; project: Project; setSt: (s: State) => void; refresh: () => Promise<void>; openCard: (id: number) => void;
}) {
  const [dragging, setDragging] = useState<Card>();
  const [form, setForm] = useState<{ kind: 'card'; lane: Lane } | { kind: 'lane' }>();
  const [menu, setMenu] = useState<{ x: number; y: number; card: Card }>();
  const [showArchive, setShowArchive] = useState(false);
  const [moving, setMoving] = useState<Card>();
  const [activeLane, setActiveLane] = useState(0);
  const mobile = useIsMobile();
  const sensors = useSensors(
    useSensor(MouseSensor, { activationConstraint: { distance: 6 } }),
    useSensor(TouchSensor, { activationConstraint: { delay: 220, tolerance: 8 } }),
  );
  const lanes = st.lanes.filter((l) => l.project_id === project.id).sort((a, b) => a.pos - b.pos);
  const archived = st.cards.filter((c) => c.project_id === project.id && c.archived);
  const cardsOf = (laneId: number) => st.cards.filter((c) => c.lane_id === laneId && !c.archived).sort((a, b) => a.pos - b.pos);

  const onDragEnd = async ({ active, over }: DragEndEvent) => {
    setDragging(undefined);
    if (!over || active.id === over.id) return;
    const cardId = Number(String(active.id).split(':')[1]);
    const [type, rawId] = String(over.id).split(':');
    const target = Number(rawId);
    const overCard = type === 'card' ? st.cards.find((c) => c.id === target) : undefined;
    const laneId = overCard ? overCard.lane_id : target;
    const siblings = cardsOf(laneId).filter((c) => c.id !== cardId);
    let pos: number;
    if (overCard) {
      const idx = siblings.findIndex((c) => c.id === overCard.id);
      pos = idx > 0 ? (siblings[idx - 1].pos + overCard.pos) / 2 : overCard.pos - 1;
    } else {
      pos = (siblings.at(-1)?.pos ?? Date.now() / 1000) + 1;
    }
    await moveCard(cardId, laneId, pos);
  };

  const moveCard = async (cardId: number, laneId: number, pos?: number) => {
    const target = pos ?? (cardsOf(laneId).filter((c) => c.id !== cardId).at(-1)?.pos ?? Date.now() / 1000) + 1;
    setSt({ ...st, cards: st.cards.map((c) => (c.id === cardId ? { ...c, lane_id: laneId, pos: target } : c)) });
    try { await api('POST', `/cards/${cardId}/move`, { lane_id: laneId, pos: target }); } catch (e) { tell((e as Error).message); }
    refresh();
  };

  const currentLane = lanes.find((l) => l.id === activeLane) ?? lanes[0];

  return (
    <div className="flex h-full flex-col">
      <header className="flex flex-wrap items-center gap-2 border-b border-line px-3 py-3 md:gap-3 md:px-4">
        <h1 className="truncate text-lg font-semibold">{project.name}</h1>
        <span className="hidden truncate text-xs text-neutral-500 xl:block">{project.path}</span>
        <button onClick={async () => { await api('POST', `/projects/${project.id}/reindex`); refresh(); }} disabled={project.wiki_status === 'indexing'}
          title="Reindexar a wiki do projeto (RAG)" className="shrink-0 whitespace-nowrap rounded-lg border border-line px-2 py-1 text-xs text-neutral-400 hover:bg-card disabled:opacity-60">
          {project.wiki_status === 'indexing' ? 'Wiki: indexando…' : project.wiki_status === 'ready' ? `Wiki: ${project.wiki_files.toLocaleString('pt-BR')} arquivos ↻` : project.wiki_status === 'error' ? 'Wiki: erro ↻' : 'Indexar wiki'}
        </button>
        <div className="ml-auto flex gap-2">
          <button aria-label="Arquivados" onClick={() => setShowArchive(true)} className="flex shrink-0 items-center gap-2 whitespace-nowrap rounded-xl border border-line px-3 py-2 text-sm text-neutral-300 hover:bg-card"><Archive size={14} /><span className="hidden sm:inline">Arquivados</span>{archived.length > 0 && ` (${archived.length})`}</button>
          <button onClick={() => setForm({ kind: 'lane' })} className="shrink-0 whitespace-nowrap rounded-xl border border-line px-3 py-2 text-sm text-neutral-300 hover:bg-card">+ Categoria</button>
          <ShimmerButton onClick={() => setForm({ kind: 'card', lane: lanes[0] })}><Plus size={16} /> Novo card</ShimmerButton>
        </div>
      </header>
      {mobile ? (
        <>
          <nav className="flex shrink-0 gap-2 overflow-x-auto border-b border-line px-3 py-2">
            {lanes.map((l) => (
              <button key={l.id} onClick={() => setActiveLane(l.id)}
                className={`flex shrink-0 items-center gap-2 rounded-full border px-3 py-1.5 text-sm ${l.id === currentLane?.id ? 'border-accent text-accent' : 'border-line text-neutral-300'}`}>
                <span className={`size-2 rounded-full ${KIND[l.kind].dot}`} />{l.name}<span className="text-xs text-neutral-500">{cardsOf(l.id).length}</span>
              </button>
            ))}
          </nav>
          <div className="min-h-0 flex-1 space-y-2 overflow-y-auto p-3">
            {currentLane && cardsOf(currentLane.id).map((c) => (
              <div key={c.id} className="relative">
                <CardView card={c} onOpen={() => openCard(c.id)} />
                <button aria-label="Mover card" onClick={() => setMoving(c)}
                  className="absolute right-2 top-2 grid size-9 place-items-center rounded-lg border border-line bg-panel text-neutral-300 active:scale-95">
                  <ArrowRightLeft size={16} />
                </button>
              </div>
            ))}
            {currentLane && cardsOf(currentLane.id).length === 0 && <p className="py-10 text-center text-sm text-neutral-500">Nenhum card aqui.</p>}
            {currentLane && <button onClick={() => setForm({ kind: 'card', lane: currentLane })} className="w-full rounded-xl border border-dashed border-line py-3 text-sm text-neutral-400">+ Novo card em {currentLane.name}</button>}
          </div>
        </>
      ) : (
      <DndContext sensors={sensors} onDragStart={(e) => setDragging(st.cards.find((c) => `card:${c.id}` === e.active.id))} onDragEnd={onDragEnd} onDragCancel={() => setDragging(undefined)}>
        <div className="flex min-h-0 flex-1 gap-3 overflow-x-auto p-4">
          {lanes.map((l) => (
            <LaneView key={l.id} lane={l} cards={cardsOf(l.id)} onOpen={openCard} onMenu={(e, card) => { e.preventDefault(); setMenu({ x: e.clientX, y: e.clientY, card }); }} onAdd={() => setForm({ kind: 'card', lane: l })}
              onDelete={async () => { try { await api('DELETE', `/lanes/${l.id}`); refresh(); } catch (e) { tell((e as Error).message); } }} />
          ))}
        </div>
        <DragOverlay>{dragging && <CardView card={dragging} ghost />}</DragOverlay>
      </DndContext>
      )}

      {moving && (
        <Dialog title={`Mover "${moving.title}"`} onClose={() => setMoving(undefined)}>
          <ul className="space-y-2">
            {lanes.filter((l) => l.id !== moving.lane_id).map((l) => (
              <li key={l.id}>
                <button onClick={() => { const c = moving; setMoving(undefined); setActiveLane(l.id); moveCard(c.id, l.id); }}
                  className="flex w-full items-center gap-3 rounded-xl border border-line px-4 py-3 text-left active:bg-card">
                  <span className={`size-2.5 rounded-full ${KIND[l.kind].dot}`} />
                  <span className="flex-1 text-sm font-medium">{l.name}</span>
                  <span className="text-xs text-neutral-500">{KIND[l.kind].label}</span>
                </button>
              </li>
            ))}
          </ul>
        </Dialog>
      )}
      {menu && <CardMenu {...menu} onClose={() => setMenu(undefined)} refresh={refresh} />}
      {showArchive && (
        <Dialog title="Arquivados" onClose={() => setShowArchive(false)}>
          <ul className="max-h-96 space-y-2 overflow-y-auto">
            {archived.length === 0 && <li className="text-sm text-neutral-500">Nada arquivado.</li>}
            {archived.map((c) => (
              <li key={c.id} className="flex items-center gap-2 rounded-lg border border-line p-2 text-sm">
                <button className="min-w-0 flex-1 truncate text-left" onClick={() => { setShowArchive(false); openCard(c.id); }}>{c.title}</button>
                <button aria-label="Restaurar" title="Restaurar" onClick={async () => { await api('POST', `/cards/${c.id}/archive`, { archived: false }); refresh(); }}><ArchiveRestore size={16} /></button>
                <button aria-label="Excluir" title="Excluir" className="text-neutral-500 hover:text-red-400" onClick={async () => { if (await ask(`Excluir "${c.title}" definitivamente?`)) { await api('DELETE', `/cards/${c.id}`); refresh(); } }}><Trash2 size={16} /></button>
              </li>
            ))}
          </ul>
        </Dialog>
      )}
      {form?.kind === 'card' && (
        <FormDialog title={`Novo card em ${form.lane.name}`} submit="Criar" onClose={() => setForm(undefined)}
          fields={[{ name: 'title', label: 'Título' }, { name: 'body', label: 'Descrição', area: true }]}
          onSubmit={async (v) => { await api('POST', '/cards', { ...v, lane_id: form.lane.id }); await refresh(); }} />
      )}
      {form?.kind === 'lane' && (
        <FormDialog title="Nova categoria" submit="Criar" onClose={() => setForm(undefined)}
          fields={[{ name: 'name', label: 'Nome' }]}
          onSubmit={async (v) => { await api('POST', '/lanes', { ...v, project_id: project.id }); await refresh(); }} />
      )}
    </div>
  );
}

function CardMenu({ x, y, card, onClose, refresh }: { x: number; y: number; card: Card; onClose: () => void; refresh: () => Promise<void> }) {
  const run = async (fn: () => Promise<unknown>) => {
    onClose();
    try { await fn(); await refresh(); } catch (e) { tell((e as Error).message); }
  };
  const item = 'flex w-full items-center gap-2 px-3 py-2 text-left text-sm hover:bg-card';
  return (
    <div className="fixed inset-0 z-50" onClick={onClose} onContextMenu={(e) => { e.preventDefault(); onClose(); }}>
      <div className="absolute w-52 overflow-hidden rounded-xl border border-line bg-panel py-1 shadow-2xl"
        style={{ left: Math.min(x, window.innerWidth - 216), top: Math.min(y, window.innerHeight - 140) }}>
        <button className={item} onClick={() => run(() => api('POST', `/cards/${card.id}/open`, { app: 'finder' }))}><FolderOpen size={14} /> Abrir pasta</button>
        <button className={item} onClick={() => run(() => api('POST', `/cards/${card.id}/open`, { app: 'vscode' }))}><SquareTerminal size={14} /> Abrir no VS Code</button>
        <button className={item} onClick={() => run(() => api('POST', `/cards/${card.id}/archive`, { archived: true }))}><Archive size={14} /> Arquivar</button>
      </div>
    </div>
  );
}
