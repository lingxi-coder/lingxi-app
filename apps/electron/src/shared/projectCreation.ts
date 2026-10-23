/** Native folder selection is staged until the user confirms the project dialog. */
export interface ProjectFolderSelection {
  id: string;
  path: string;
  /** A new named child will be created only after confirmation. */
  createDirectory?: true;
}

export type ProjectCreationPurpose = 'project' | 'local-app';

export const CH_PROJECT_FOLDER_PICK = 'lingxi:project:pick-folder';
export const CH_PROJECT_APPS_DEFAULT = 'lingxi:project:apps-default';
export const CH_PROJECT_CREATE = 'lingxi:project:create';
