// Icons from Material Icon Theme (MIT); see assets/file-icons/LICENSE.
const icons = {
  c: new URL('../assets/file-icons/c.svg', import.meta.url).href,
  cpp: new URL('../assets/file-icons/cpp.svg', import.meta.url).href,
  csharp: new URL('../assets/file-icons/csharp.svg', import.meta.url).href,
  css: new URL('../assets/file-icons/css.svg', import.meta.url).href,
  dart: new URL('../assets/file-icons/dart.svg', import.meta.url).href,
  docker: new URL('../assets/file-icons/docker.svg', import.meta.url).href,
  elixir: new URL('../assets/file-icons/elixir.svg', import.meta.url).href,
  file: new URL('../assets/file-icons/file.svg', import.meta.url).href,
  git: new URL('../assets/file-icons/git.svg', import.meta.url).href,
  go: new URL('../assets/file-icons/go.svg', import.meta.url).href,
  graphql: new URL('../assets/file-icons/graphql.svg', import.meta.url).href,
  html: new URL('../assets/file-icons/html.svg', import.meta.url).href,
  java: new URL('../assets/file-icons/java.svg', import.meta.url).href,
  javascript: new URL('../assets/file-icons/javascript.svg', import.meta.url).href,
  json: new URL('../assets/file-icons/json.svg', import.meta.url).href,
  kotlin: new URL('../assets/file-icons/kotlin.svg', import.meta.url).href,
  lua: new URL('../assets/file-icons/lua.svg', import.meta.url).href,
  markdown: new URL('../assets/file-icons/markdown.svg', import.meta.url).href,
  php: new URL('../assets/file-icons/php.svg', import.meta.url).href,
  powershell: new URL('../assets/file-icons/powershell.svg', import.meta.url).href,
  python: new URL('../assets/file-icons/python.svg', import.meta.url).href,
  react: new URL('../assets/file-icons/react.svg', import.meta.url).href,
  ruby: new URL('../assets/file-icons/ruby.svg', import.meta.url).href,
  rust: new URL('../assets/file-icons/rust.svg', import.meta.url).href,
  scala: new URL('../assets/file-icons/scala.svg', import.meta.url).href,
  svelte: new URL('../assets/file-icons/svelte.svg', import.meta.url).href,
  swift: new URL('../assets/file-icons/swift.svg', import.meta.url).href,
  terraform: new URL('../assets/file-icons/terraform.svg', import.meta.url).href,
  toml: new URL('../assets/file-icons/toml.svg', import.meta.url).href,
  typescript: new URL('../assets/file-icons/typescript.svg', import.meta.url).href,
  vue: new URL('../assets/file-icons/vue.svg', import.meta.url).href,
  xml: new URL('../assets/file-icons/xml.svg', import.meta.url).href,
  yaml: new URL('../assets/file-icons/yaml.svg', import.meta.url).href,
} as const;
type IconType = keyof typeof icons;
const extensions: Record<string, IconType> = {
  swift: 'swift', ts: 'typescript', mts: 'typescript', cts: 'typescript', tsx: 'react', jsx: 'react',
  js: 'javascript', mjs: 'javascript', cjs: 'javascript', py: 'python', rs: 'rust', go: 'go',
  java: 'java', kt: 'kotlin', kts: 'kotlin', c: 'c', h: 'c', cc: 'cpp', cpp: 'cpp', cxx: 'cpp', hpp: 'cpp', cs: 'csharp',
  yml: 'yaml', yaml: 'yaml', json: 'json', jsonc: 'json', html: 'html', htm: 'html', css: 'css',
  md: 'markdown', mdx: 'markdown', xml: 'xml', toml: 'toml', dart: 'dart',
  php: 'php', rb: 'ruby', lua: 'lua', scala: 'scala', ex: 'elixir', exs: 'elixir',
  vue: 'vue', svelte: 'svelte', graphql: 'graphql', gql: 'graphql', tf: 'terraform', ps1: 'powershell',
};
export function fileTypeIcon(path: string): string {
  const name = path.split(/[\\/]/).pop()?.toLowerCase() ?? '';
  if (name === 'dockerfile' || name.startsWith('dockerfile.')) return icons.docker;
  if (name === '.gitignore' || name === '.gitattributes' || name === '.gitmodules') return icons.git;
  const extension = name.includes('.') ? name.slice(name.lastIndexOf('.') + 1) : '';
  return icons[extensions[extension] ?? 'file'];
}
