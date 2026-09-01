import { useEffect, useState } from 'react';
import type { SkillDto } from '@lingxi/bridge-client';
import { Card, FieldProvenanceNotice, Row } from '../rows';
import { useT } from '../../../theme/ThemeContext';
import { Toggle } from '../primitives';
import type { EditableLayer, PageContentProps } from '../SettingsScreen';
import { boolFromLayer } from '../layerFields';
import { ghostButtonStyle } from './ghostButton';

export interface SkillsPageModel {
  skills: SkillDto[];
  /**
   * Documents, at the type level, which row the layer switcher can actually
   * move — `'syncClaudeAiSkills only'` is a literal, not a computed value,
   * because there is nothing else on this page for a layer to affect.
   */
  layerAffects: 'syncClaudeAiSkills only';
}

/**
 * The pinned decision this task's brief names directly: the skills LIST is
 * discovered from `skills/` directories (`ClientEvent::Skills`, Task 7),
 * not read per-layer — its shape structurally cannot depend on `layer`
 * because this function never reads `input.layer` to compute `.skills`.
 * Only `syncClaudeAiSkills` (a real settings key) is layered; `layerAffects`
 * says so for the page to render as a visible caveat rather than a silent
 * assumption a user could reasonably get wrong ("I switched layers, where
 * did my skill go?").
 */
export function skillsPageModel(input: { layer: EditableLayer; skills?: SkillDto[] }): SkillsPageModel {
  return { skills: input.skills ?? [], layerAffects: 'syncClaudeAiSkills only' };
}

/**
 * Skills is `layered` per `nav.ts`, but the layer switcher affects exactly
 * ONE row on this page (`syncClaudeAiSkills`) — the skills list itself is a
 * directory-discovery VIEW (`refresh_listings{skills}` →
 * `ClientEvent::Skills`, Task 7), unrelated to any settings file layer. The
 * "还没接过引擎" empty-listing case and "engine has zero skills" both render
 * as the same empty state; this page has no way to tell them apart without
 * a dedicated loading flag, and guessing would be worse than not guessing.
 */
export function Skills({ bridge, snapshot, editingLayer, onJumpToLayer }: PageContentProps) {
  const t = useT();
  const model = skillsPageModel({ layer: editingLayer, skills: bridge.skillsEvent?.skills });
  const syncEnabled = boolFromLayer(snapshot, editingLayer, 'syncClaudeAiSkills');

  const [syncSaving, setSyncSaving] = useState(false);
  const [syncError, setSyncError] = useState<string | null>(null);
  const [reloading, setReloading] = useState(false);
  const [reloadError, setReloadError] = useState<string | null>(null);

  useEffect(() => { void bridge.refreshSkills(); }, [bridge.refreshSkills]);

  const handleToggleSync = (next: boolean) => {
    setSyncSaving(true);
    setSyncError(null);
    void bridge.updateEngineSettings(editingLayer, { syncClaudeAiSkills: next })
      .catch((cause) => setSyncError(cause instanceof Error ? cause.message : '无法保存 syncClaudeAiSkills。'))
      .finally(() => setSyncSaving(false));
  };

  const handleReload = () => {
    setReloading(true);
    setReloadError(null);
    void bridge.runSlashCommand('/reload-skills')
      .then(() => bridge.refreshSkills())
      .catch((cause) => setReloadError(cause instanceof Error ? cause.message : '无法重新加载 skills。'))
      .finally(() => setReloading(false));
  };

  return (
    <>
      <Card title="已发现的 Skills">
        <div style={{ padding: '10px 18px 0', fontSize: 12, color: t.text3 }}>
          这份列表来自磁盘目录扫描，与上方的层切换器无关——切换层不会改变它。只有下面的「同步 Claude.ai skills」一项真的按层写入。
        </div>
        {model.skills.length === 0 && (
          <div style={{ padding: '14px 18px', color: t.text4, fontSize: 12.5 }}>没有发现 skill，或引擎尚未上报。</div>
        )}
        {model.skills.map((skill) => (
          <Row key={skill.source_dir} align="center" title={skill.name} desc={<span className="mono" style={{ fontSize: 11.5 }}>{skill.source_dir}</span>}>{null}</Row>
        ))}
        <Row title="重新加载" desc="等价于运行 /reload-skills。" align="center">
          <button type="button" disabled={reloading} onClick={handleReload} style={ghostButtonStyle(t, reloading)}>{reloading ? '重新加载中…' : '重新加载'}</button>
        </Row>
        {reloadError && <Row title="错误" align="center"><span role="alert" style={{ color: t.danger, fontSize: 12 }}>{reloadError}</span></Row>}
      </Card>

      <Card title="Claude.ai 同步">
        <FieldProvenanceNotice snapshot={snapshot} fieldKey="syncClaudeAiSkills" editingLayer={editingLayer} onJumpToLayer={onJumpToLayer} />
        <Row title="syncClaudeAiSkills" desc="是否同步 Claude.ai 上的 skills。这是本页唯一按层写入的设置。" align="center">
          <Toggle value={syncEnabled} onChange={syncSaving ? () => undefined : handleToggleSync} />
        </Row>
        {syncError && <Row title="错误" align="center"><span role="alert" style={{ color: t.danger, fontSize: 12 }}>{syncError}</span></Row>}
      </Card>
    </>
  );
}
