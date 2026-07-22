import React, { useEffect } from 'react';
import ReactMarkdown from 'react-markdown';
import remarkGfm from 'remark-gfm';
import { useStore, PRESETS } from '../store.js';
import { HELP } from '../refFormat.js';
import { PARAM_DOCS, SCALAR_FIELDS, EXTRA_FIELDS } from './ParamsPanel.jsx';
import userGuideMd from '../docs/userGuide.md?raw';

// Parameter reference is rendered from the *same* objects that power the
// live UI -- HELP backs the `?` tooltips in ResultPanel's Reference section,
// and PARAM_DOCS/SCALAR_FIELDS/EXTRA_FIELDS back the "?" popouts in
// ParamsPanel -- rather than duplicated as hand-written prose here. One
// source of truth, so the tooltip/popout text and this doc can't silently
// drift apart the way two independent descriptions of the same field
// eventually would.
const PARAM_ORDER = ['frc', 'fow', 'bearing', 'dnp', 'lfrcnp', 'offset'];
const PARAM_TITLE = {
  frc: 'FRC (Functional Road Class)', fow: 'FOW (Form of Way)', bearing: 'Bearing',
  dnp: 'DNP (Distance to Next Point)', lfrcnp: 'LFRCNP', offset: 'Positive / Negative offset',
};

// Fields whose preset value is worth calling out in the at-a-glance
// Permissive/Default/Strict comparison table -- the ones that most directly
// trade recall (does it decode at all) against precision (is the match
// actually correct), rather than every one of the ~18 tunable fields.
const PRESET_COMPARE_FIELDS = [
  { key: 'candidate_search_radius_m',    label: 'Search radius',      unit: 'm' },
  { key: 'max_bearing_deviation_deg',    label: 'Max bearing dev.',   unit: '°' },
  { key: 'dnp_tolerance_pct',            label: 'DNP tolerance',      unit: '' },
  { key: 'lfrcnp_tolerance',             label: 'LFRCNP tolerance',   unit: 'steps' },
  { key: 'max_candidates_per_lrp',       label: 'Max candidates/LRP', unit: '' },
];

// ── Heading-anchor plumbing ───────────────────────────────────────────────────
//
// react-markdown doesn't assign `id`s to headings on its own (that normally
// needs a rehype plugin) -- rather than pull in another dependency, `slugify`
// + a custom `h2`/`h3` renderer compute the same id a hand-written TOC link
// can target, and a custom `a` renderer intercepts in-page `#anchor` clicks
// to scroll smoothly within this page's own scroll container rather than
// falling through to the browser's default `location.hash` navigation --
// which would otherwise fight with MapLibre's own `hash: true` URL state
// on the app underneath.
function slugify(text) {
  return text.toLowerCase().trim().replace(/[^\w\s-]/g, '').replace(/\s+/g, '-');
}

function flattenText(node) {
  if (node == null || typeof node === 'boolean') return '';
  if (typeof node === 'string' || typeof node === 'number') return String(node);
  if (Array.isArray(node)) return node.map(flattenText).join('');
  if (React.isValidElement(node)) return flattenText(node.props.children);
  return '';
}

function headingRenderer(Tag) {
  return function Heading({ children }) {
    const id = slugify(flattenText(children));
    return <Tag id={id}>{children}</Tag>;
  };
}

function AnchorLink({ href, children }) {
  if (href?.startsWith('#')) {
    return (
      <a
        href={href}
        onClick={(e) => {
          e.preventDefault();
          document.getElementById(href.slice(1))?.scrollIntoView({ behavior: 'smooth', block: 'start' });
        }}
      >{children}</a>
    );
  }
  return <a href={href} target="_blank" rel="noreferrer">{children}</a>;
}

const MD_COMPONENTS = { h2: headingRenderer('h2'), h3: headingRenderer('h3'), a: AnchorLink };

// ── Dynamic parameter-detail block (mirrors ParamDocPopout's content) ────────

function ParamDetail({ label, unit, doc }) {
  if (!doc) return null;
  return (
    <div className="docs-param-detail">
      <div className="docs-param-detail-title">{label}{unit ? ` (${unit})` : ''}</div>
      <p>{doc.what}</p>
      <p><span className="param-doc-dir-up">▲ Increasing</span> — {doc.increase}</p>
      <p><span className="param-doc-dir-down">▼ Decreasing</span> — {doc.decrease}</p>
    </div>
  );
}

export default function DocumentationPage() {
  const { route, closeDocs } = useStore();
  const active = route === 'docs';

  // Escape closes back to the app, same as every other floating panel/popout
  // in this codebase (see e.g. ParamDocPopout) -- consistent expectation even
  // though this is a full page rather than a small overlay.
  useEffect(() => {
    if (!active) return;
    const onKey = (e) => { if (e.key === 'Escape') closeDocs(); };
    document.addEventListener('keydown', onKey);
    return () => document.removeEventListener('keydown', onKey);
  }, [active, closeDocs]);

  if (!active) return null;

  return (
    <div className="docs-page">
      <div className="docs-page-header">
        <button className="docs-page-back" onClick={closeDocs} title="Back to the app (Esc)">← Back to app</button>
        <span className="docs-page-title">Documentation</span>
      </div>
      <div className="docs-page-body">
        <ReactMarkdown remarkPlugins={[remarkGfm]} components={MD_COMPONENTS}>{userGuideMd}</ReactMarkdown>

        {/* ── Dynamic: full decode-parameter reference (What / ▲ / ▼ for every
            tunable field), generated from the same PARAM_DOCS object that
            drives the "?" popouts in the live Parameters panel. ── */}
        {SCALAR_FIELDS.map(({ key, label, unit }) => (
          <ParamDetail key={key} label={label} unit={unit} doc={PARAM_DOCS[key]} />
        ))}
        {Object.entries(EXTRA_FIELDS).map(([key, { label, unit }]) => (
          <ParamDetail key={key} label={label} unit={unit} doc={PARAM_DOCS[key]} />
        ))}

        <h3 id="presets">Presets</h3>
        <p>
          The three built-in presets (Permissive / Default / Strict) set every field above at once,
          trading recall (does it decode at all) against precision (is the match actually correct).
          A few of the most consequential fields, for comparison:
        </p>
        <table className="docs-preset-table">
          <thead>
            <tr>
              <th>Field</th>
              {Object.keys(PRESETS).map(name => <th key={name}>{name}</th>)}
            </tr>
          </thead>
          <tbody>
            {PRESET_COMPARE_FIELDS.map(({ key, label, unit }) => (
              <tr key={key}>
                <td>{label}</td>
                {Object.keys(PRESETS).map(name => (
                  <td key={name}>{PRESETS[name][key]}{unit ? ` ${unit}` : ''}</td>
                ))}
              </tr>
            ))}
          </tbody>
        </table>

        <h2 id="candidate-matching-field-reference">Candidate Matching Field Reference</h2>
        <p>
          What each LRP attribute means and how the decoder uses it to accept or reject a candidate
          segment — the same text shown by the <code>?</code> icons next to these fields in the
          Results panel's Reference section.
        </p>
        <dl className="docs-param-list">
          {PARAM_ORDER.map(key => (
            <React.Fragment key={key}>
              <dt>{PARAM_TITLE[key]}</dt>
              <dd>{HELP[key]}</dd>
            </React.Fragment>
          ))}
        </dl>
      </div>
    </div>
  );
}
