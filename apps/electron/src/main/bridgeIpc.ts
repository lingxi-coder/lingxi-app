import { createRequire } from 'node:module';

export const CH_SEND_PROMPT = 'lingxi:sendPrompt';

export const CH_APPROVE = 'lingxi:approve';

export const CH_DENY = 'lingxi:deny';

export const CH_APPROVE_COMPUTER_ACCESS = 'lingxi:approveComputerAccess';

export const CH_DENY_COMPUTER_ACCESS = 'lingxi:denyComputerAccess';

export const CH_ANSWER_ASK_USER_QUESTION = 'lingxi:answerAskUserQuestion';

export const CH_CANCEL_ASK_USER_QUESTION = 'lingxi:cancelAskUserQuestion';

export const CH_CANCEL = 'lingxi:cancel';

export const CH_COMMAND = 'lingxi:command';

export const CH_MOD_UI_CONTROL = 'lingxi:modUi:control';

export const CH_MOD_UI_OPERATION = 'lingxi:modUi:operation';

export const CH_MOD_UI_FRAME = 'lingxi:modUi:frame';

export const CH_MOD_UI_INVALIDATE = 'lingxi:modUi:invalidate';

export const CH_CONNECTION_STATE = 'lingxi:connectionState';

export const CH_EVENT = 'lingxi:event';

export const CH_EVENT_REPLAY = 'lingxi:event:replay';

export const CH_PERMISSION = 'lingxi:permission';

export const CH_COMPUTER_ACCESS = 'lingxi:computerAccess';

export const CH_STATE_CHANGED = 'lingxi:connectionStateChanged';

export const require = createRequire(import.meta.url);

const electronModule = require('electron');

export const ipcMain = (typeof electronModule === 'string' ? undefined : electronModule.ipcMain) ?? {
  handle: () => { throw new Error('ipcMain is unavailable outside Electron'); },
  removeHandler: () => undefined,
};
