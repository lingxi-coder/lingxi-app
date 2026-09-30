import assert from 'node:assert/strict';
import test from 'node:test';
import { formatCurrency, pickLocaleText } from '../src/utils/locale';

test('formats localized copy and currency signs correctly', () => {
  assert.equal(pickLocaleText('zh', { en: 'Usage', zh: '用量' }), '用量');
  assert.equal(formatCurrency(19, 'global'), '$19.00');
  assert.equal(formatCurrency(-26.88, 'global'), '-$26.88');
  assert.equal(formatCurrency(-10, 'china'), '-CN¥72');
});
