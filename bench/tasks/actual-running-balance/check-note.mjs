import fs from 'node:fs';
import matter from 'gray-matter';
import { categoryAutocorrections, categoryOrder } from './packages/ci-actions/src/release-notes/util.mjs';

// With `node -e ... -- <paths>`, argv[1] is the first path: there is no script filename.
const notes = process.argv.slice(1).map(path => matter(fs.readFileSync(path, 'utf8')));
const valid = ({ data, content }) => {
  const category = categoryAutocorrections[data.category] ?? data.category;
  const body = content.trim();
  return categoryOrder.includes(category) && Array.isArray(data.authors) &&
    body.length > 0 && !body.includes('\n');
};
if (!notes.length || !notes.every(valid) || !notes.some(note => note.data.authors.includes('jouet-bench'))) {
  console.error('The release note must follow the project format and credit jouet-bench.');
  process.exitCode = 1;
}
