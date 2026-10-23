/** Native folder selection is staged until the user confirms the project dialog. */
export interface ProjectFolderSelection {
  id: string;
  path: string;
}

export const CH_PROJECT_FOLDER_PICK = 'lingxi:project:pick-folder';
export const CH_PROJECT_CREATE = 'lingxi:project:create';
