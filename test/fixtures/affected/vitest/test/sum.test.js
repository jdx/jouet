import { expect, test } from 'vitest'
import { sum } from '../src/sum.js'

test('sum', () => expect(sum(1, 2)).toBe(3))
