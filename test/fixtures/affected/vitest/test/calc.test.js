import { expect, test } from 'vitest'
import { twice } from '../src/calc.js'

test('twice', () => expect(twice(2)).toBe(4))
