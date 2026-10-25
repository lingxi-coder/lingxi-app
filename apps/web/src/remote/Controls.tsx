import { useEffect, useState } from 'react';
import type { ConversationControlsDto, PermissionModeId, ReasoningSelectionDto } from '@lingxi/bridge-client';
import type { ModelsListResult } from '@lingxi/bridge-client/remote';
import type { Locale } from '../utils/locale';
import { selectablePermissionModes, type RemoteClient, type SessionController } from './remoteClient';
import { t, type StringKey } from './strings';
import { displayText } from './text';

const MODE_LABELS: Partial<Record<PermissionModeId, StringKey>> = {
  default: 'modeDefault',
  acceptEdits: 'modeAcceptEdits',
  plan: 'modePlan',
  auto: 'modeAuto',
  dontAsk: 'modeDontAsk',
};

function effortLabel(locale: Locale, selection: ReasoningSelectionDto): string {
  switch (selection.type) {
    case 'automatic':
      return t(locale, 'effortAutomatic');
    case 'disabled':
      return t(locale, 'effortDisabled');
    case 'enabled':
      return t(locale, 'effortEnabled');
    case 'level':
      return displayText(selection.id, 30);
    case 'token_budget':
      return t(locale, 'effortTokens', { tokens: selection.tokens });
  }
}

interface ControlsProps {
  locale: Locale;
  client: RemoteClient;
  controller: SessionController;
  controls: ConversationControlsDto | undefined;
  disabled: boolean;
}

/** Model, reasoning effort and permission mode of the session. `bypassPermissions` is never listed. */
export function Controls({ locale, client, controller, controls, disabled }: ControlsProps) {
  const [models, setModels] = useState<ModelsListResult | null>(null);
  const [failed, setFailed] = useState(false);

  useEffect(() => {
    let live = true;
    if (!disabled) client.models().then((result) => live && setModels(result), () => undefined);
    return () => {
      live = false;
    };
  }, [client, disabled]);

  const apply = (action: Promise<void>): void => {
    setFailed(false);
    action.catch(() => setFailed(true));
  };

  if (controls === undefined) return null;
  const modelNames = new Map((models?.details ?? []).map((detail) => [detail.reference, displayText(detail.display_name, 60)]));
  const modelList = models?.models ?? [];
  const current = controls.qualified_model;
  const options = modelList.includes(current) || current === '' ? modelList : [current, ...modelList];
  const reasoning = controls.reasoning;
  const reasoningOptions = reasoning.spec.options;
  const modes = selectablePermissionModes(controls);

  return (
    <div className="rc-controls">
      {options.length > 0 ? (
        <label>
          <span>{t(locale, 'model')}</span>
          <select value={current} disabled={disabled} onChange={(event) => apply(controller.setModel(event.target.value))}>
            {options.map((model) => (
              <option key={model} value={model}>
                {modelNames.get(model) || displayText(model, 80)}
              </option>
            ))}
          </select>
        </label>
      ) : null}
      {reasoningOptions.length > 1 ? (
        <label>
          <span>{t(locale, 'effort')}</span>
          <select
            value={JSON.stringify(reasoning.requested)}
            disabled={disabled || !reasoning.spec.editable}
            onChange={(event) => apply(controller.setReasoning(JSON.parse(event.target.value) as ReasoningSelectionDto))}
          >
            {reasoningOptions.map((option) => (
              <option key={JSON.stringify(option.selection)} value={JSON.stringify(option.selection)}>
                {effortLabel(locale, option.selection)}
              </option>
            ))}
          </select>
        </label>
      ) : null}
      {modes.length > 0 ? (
        <label>
          <span>{t(locale, 'permissionMode')}</span>
          <select value={controls.permission.requested} disabled={disabled} onChange={(event) => apply(controller.setPermissionMode(event.target.value as PermissionModeId))}>
            {!modes.includes(controls.permission.requested) ? <option value={controls.permission.requested} disabled>{controls.permission.requested}</option> : null}
            {modes.map((mode) => (
              <option key={mode} value={mode}>
                {MODE_LABELS[mode] ? t(locale, MODE_LABELS[mode]!) : mode}
              </option>
            ))}
          </select>
        </label>
      ) : null}
      {failed ? <p className="rc-note rc-danger">{t(locale, 'controlsFailed')}</p> : null}
    </div>
  );
}
